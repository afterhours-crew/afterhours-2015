// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Finite cold-compilation batches. Results retain source order regardless of
//! completion order. Join every worker, including after an error, before return.
use nfs_content::Error;

pub const MAX_WORKERS: usize = 4;

fn configured(requested: Option<&str>, available: usize) -> Result<usize, Error> {
    match requested {
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|v| (1..=MAX_WORKERS).contains(v))
            .ok_or(Error::Cache("AFTERHOURS_PREPARATION_WORKERS must be 1..4")),
        None => Ok(available.clamp(1, 2)),
    }
}

pub fn workers() -> Result<usize, Error> {
    let requested = std::env::var("AFTERHOURS_PREPARATION_WORKERS")
        .map(Some)
        .or_else(|e| match e {
            std::env::VarError::NotPresent => Ok(None),
            std::env::VarError::NotUnicode(_) => {
                Err(Error::Cache("invalid preparation worker setting"))
            }
        })?;
    configured(
        requested.as_deref(),
        std::thread::available_parallelism().map_or(1, usize::from),
    )
}

pub fn ordered<T: Sync, U: Send, W: Send>(
    inputs: &[T],
    workers: usize,
    mut fork: impl FnMut() -> W,
    compile: impl Fn(&mut W, &T) -> Result<U, Error> + Sync,
) -> Result<Vec<U>, Error> {
    if !(1..=MAX_WORKERS).contains(&workers)
        || inputs.len() > nfs_content::events::discovery::MAX_EVENTS
    {
        return Err(Error::Cache("parallel preparation bound"));
    }
    if inputs.is_empty() {
        return Ok(vec![]);
    }
    let workers = workers.min(inputs.len());
    if workers == 1 {
        let mut state = fork();
        return inputs
            .iter()
            .map(|input| compile(&mut state, input))
            .collect();
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        let mut error = None;
        for worker in 0..workers {
            let mut state = fork();
            let compile = &compile;
            match std::thread::Builder::new()
                .name(format!("afterhours-prepare-{worker}"))
                .spawn_scoped(scope, move || {
                    inputs
                        .iter()
                        .enumerate()
                        .skip(worker)
                        .step_by(workers)
                        .map(|(ordinal, input)| {
                            compile(&mut state, input).map(|result| (ordinal, result))
                        })
                        .collect::<Result<Vec<_>, Error>>()
                }) {
                Ok(handle) => handles.push(handle),
                Err(e) => {
                    error = Some(Error::from(e));
                    break;
                }
            }
        }
        let mut results = Vec::new();
        for handle in handles {
            match handle.join() {
                Ok(Ok(mut batch)) => results.append(&mut batch),
                Ok(Err(e)) => {
                    error.get_or_insert(e);
                }
                Err(_) => {
                    error.get_or_insert(Error::Cache("preparation worker panicked"));
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        results.sort_by_key(|(ordinal, _)| *ordinal);
        Ok(results.into_iter().map(|(_, value)| value).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn workers_are_bounded_and_results_keep_source_order() {
        for workers in 1..=MAX_WORKERS {
            let rendezvous = Barrier::new(workers);
            let forks = AtomicUsize::new(0);
            let inputs: Vec<_> = (0..29).collect();
            let results = ordered(
                &inputs,
                workers,
                || {
                    forks.fetch_add(1, Ordering::Relaxed);
                    false
                },
                |first, input| {
                    if !*first {
                        *first = true;
                        rendezvous.wait();
                    }
                    std::thread::yield_now();
                    Ok(input * input)
                },
            )
            .unwrap();
            assert_eq!(results, inputs.iter().map(|v| v * v).collect::<Vec<_>>());
            assert_eq!(forks.load(Ordering::Relaxed), workers);
        }
        assert!(ordered(&[1], 5, || (), |_, v| Ok(*v)).is_err());
        assert!(ordered(&vec![0; 513], 1, || (), |_, v| Ok(*v)).is_err());
        assert_eq!(configured(None, 32).unwrap(), 2);
        for invalid in ["0", "5", "", "-1", "many"] {
            assert!(configured(Some(invalid), 8).is_err());
        }
    }

    #[test]
    fn error_or_panic_joins_all_workers_and_never_returns_a_partial_batch() {
        for panic in [false, true] {
            let rendezvous = Barrier::new(4);
            let dropped = AtomicUsize::new(0);
            struct Worker<'a>(&'a AtomicUsize);
            impl Drop for Worker<'_> {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            let result = ordered(
                &[0, 1, 2, 3],
                4,
                || Worker(&dropped),
                |_, input| {
                    rendezvous.wait();
                    if *input == 1 {
                        assert!(!panic, "constructed worker failure");
                        return Err(Error::Cache("constructed compile error"));
                    }
                    Ok(*input)
                },
            );
            assert!(result.is_err());
            assert_eq!(dropped.load(Ordering::SeqCst), 4);
        }
    }
}
