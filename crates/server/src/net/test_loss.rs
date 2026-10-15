// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Opt-in, one-shot loss at the socket edge, before transport state changes.
//! Only compiled with `network-test`, and only enabled for a raw recording.
use std::{io, path::PathBuf};
use tokio::io::AsyncReadExt;

const COMMAND: &[u8] = b"drop-incoming-2000ms";
const MAX_DROPS: u16 = 256;
const LOSS_MS: u64 = 2_000;
const POLL_MS: u64 = 200;

pub(super) struct Loss {
    trigger: Option<PathBuf>,
    next_poll: u64,
    deadline: u64,
    remaining: u16,
}

impl Loss {
    pub(super) fn new(trigger: Option<PathBuf>) -> Self {
        Self {
            trigger,
            next_poll: 0,
            deadline: 0,
            remaining: 0,
        }
    }

    pub(super) async fn poll(&mut self, now: u64) {
        if now < self.next_poll {
            return;
        }
        let Some(path) = self.trigger.as_ref() else {
            return;
        };
        self.next_poll = now.saturating_add(POLL_MS);
        let metadata = match tokio::fs::symlink_metadata(path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                tracing::warn!(?error, "diagnostic loss trigger unavailable; disabled");
                self.trigger = None;
                return;
            }
        };
        // Consume one command attempt per world connection. Never delete, write
        // or follow a known symlink. A fresh recording directory prevents a
        // previous run's command from silently affecting a later game.
        let path = self.trigger.take().expect("trigger checked above");
        if !metadata.file_type().is_file() || metadata.len() != COMMAND.len() as u64 {
            tracing::warn!("invalid diagnostic loss trigger; disabled");
            return;
        }
        let Ok(file) = tokio::fs::File::open(path).await else {
            return;
        };
        let mut bytes = Vec::new();
        if file
            .take(COMMAND.len() as u64 + 1)
            .read_to_end(&mut bytes)
            .await
            .is_err()
            || bytes != COMMAND
        {
            tracing::warn!("invalid diagnostic loss command; disabled");
            return;
        }
        self.deadline = now.saturating_add(LOSS_MS);
        self.remaining = MAX_DROPS;
        tracing::warn!(
            deadline_ms = self.deadline,
            max_packets = MAX_DROPS,
            "diagnostic incoming world packet loss armed"
        );
    }

    pub(super) fn drop_input(&mut self, now: u64) -> bool {
        if self.remaining == 0 || now >= self.deadline {
            return false;
        }
        self.remaining -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "afterhours-loss-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn command(&self, connection: u32, value: &[u8]) -> PathBuf {
            let path = self.0.join(format!("world-{connection}-loss.txt"));
            std::fs::write(&path, value).unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[tokio::test]
    async fn opt_in_is_bounded_once_and_isolated_by_connection() {
        let dir = Temp::new();
        let first = dir.command(1, COMMAND);
        let mut a = Loss::new(Some(first));
        let mut b = Loss::new(Some(dir.0.join("world-2-loss.txt")));
        let mut disabled = Loss::new(None);
        assert!(!a.drop_input(0));
        a.poll(100).await;
        b.poll(100).await;
        disabled.poll(100).await;
        for _ in 0..MAX_DROPS {
            assert!(a.drop_input(101));
        }
        assert!(!a.drop_input(101));
        assert!(!b.drop_input(101));
        assert!(!disabled.drop_input(101));
        a.poll(10_000).await;
        assert!(!a.drop_input(10_001));
        assert_eq!(
            std::fs::read(dir.0.join("world-1-loss.txt")).unwrap(),
            COMMAND
        );
    }

    #[tokio::test]
    async fn absent_trigger_can_arrive_later_but_loss_expires() {
        let dir = Temp::new();
        let mut loss = Loss::new(Some(dir.0.join("world-1-loss.txt")));
        loss.poll(0).await;
        dir.command(1, COMMAND);
        loss.poll(POLL_MS - 1).await;
        assert!(!loss.drop_input(POLL_MS - 1));
        loss.poll(POLL_MS).await;
        assert!(loss.drop_input(POLL_MS + LOSS_MS - 1));
        assert!(!loss.drop_input(POLL_MS + LOSS_MS));
        loss.poll(POLL_MS + LOSS_MS).await;
        assert!(!loss.drop_input(POLL_MS + LOSS_MS + 1));
    }

    #[tokio::test]
    async fn invalid_or_oversized_commands_never_arm() {
        for bytes in [b"".as_slice(), b"drop-outgoing-2000ms", &[b'x'; 1024]] {
            let dir = Temp::new();
            let path = dir.command(1, bytes);
            let mut loss = Loss::new(Some(path));
            loss.poll(0).await;
            dir.command(1, COMMAND);
            loss.poll(POLL_MS).await;
            assert!(!loss.drop_input(POLL_MS));
        }
    }
}
