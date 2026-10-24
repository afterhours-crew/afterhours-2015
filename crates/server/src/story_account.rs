// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Offline story edits. One preview proof, held process ownership, a complete
//! backup, then one atomic story/receipt commit. No client database writes.
use nfs_services::{
    persistent::key,
    progression::{Catalog, Settings},
    story::{
        Checkpoint, Policy, Prepared,
        editing::{Applied, Backup, Milestone, Preview},
    },
};
use nfs_storage::{
    AccountId, Error, InventoryRepository, Snapshot, SqliteRepository, Timestamp,
    tables::{Row, Table, Value},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const MAX_RULE_BYTES: u64 = 512 * 1024;
const HELP: &str = "Usage: nfs-server story --root <data-directory> --local-account <id> --progression-content <file> --preview <checkpoint>\nApply: replace --preview with --apply <checkpoint> --expected-generation <generation> --plan-id <preview hash>\nJSON only. Existing local accounts; no listeners or game are started. Unsupported restart points refuse edits. Stop the game/server before applying. Backups are under artifacts/story-backups.";

struct Rules {
    catalog: Catalog,
    settings: Settings,
    digest: String,
}
struct Proof {
    target: Checkpoint,
    expected: u64,
    plan_id: String,
}
struct Editor<'a> {
    root: &'a Path,
    account: AccountId,
    rules: &'a Rules,
    supported: bool,
}

fn digest(bytes: &[u8]) -> String {
    openssl::sha::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn hex(text: &str, size: usize) -> bool {
    text.len() == size
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn decimal(text: &str) -> Result<u64> {
    let value = text.parse::<u64>()?;
    if value.to_string() != text {
        return Err("Noncanonical decimal identifier.".into());
    }
    Ok(value)
}
fn checkpoint(text: &str) -> Result<Checkpoint> {
    if text == "after-intro" {
        return Ok(Checkpoint::AfterIntro);
    }
    let id = decimal(text)?;
    if id == 0 {
        return Err("Unknown story checkpoint.".into());
    }
    Ok(Checkpoint::Objective(id))
}
fn name(target: Checkpoint) -> String {
    match target {
        Checkpoint::AfterIntro => "after-intro".into(),
        Checkpoint::Objective(id) => id.to_string(),
    }
}
fn ordinary(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if crate::local_setup::linked(&meta) || !meta.is_dir() {
        return Err("Story data must use ordinary local directories.".into());
    }
    Ok(())
}
fn root(data: &Path) -> Result<PathBuf> {
    let data = std::path::absolute(data)?;
    if data.parent().is_none() {
        return Err("Choose a dedicated data directory.".into());
    }
    for ancestor in data.ancestors() {
        ordinary(ancestor)?;
    }
    ordinary(&data.join("artifacts"))?;
    ordinary(&data.join("artifacts/account"))?;
    Ok(data)
}
fn session_lock(root: &Path, create: bool) -> Result<Option<File>> {
    let path = root.join("launcher.lock");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if crate::local_setup::linked(&meta) || !meta.is_file() => {
            return Err("Invalid launcher ownership file.".into());
        }
        Ok(_) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    let file = File::options()
        .read(true)
        .write(true)
        .create(create)
        .truncate(false)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::Busy.into()),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let meta = std::fs::symlink_metadata(path)?;
    if crate::local_setup::linked(&meta) || !meta.is_file() || meta.len() > limit {
        return Err("Invalid or oversized story file.".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("Oversized story file.".into());
    }
    Ok(bytes)
}
fn file_digest(path: &Path) -> Result<String> {
    let meta = std::fs::symlink_metadata(path)?;
    if crate::local_setup::linked(&meta) || !meta.is_file() || meta.len() > 64 * 1024 * 1024 {
        return Err("Invalid story backup database.".into());
    }
    let mut hash = openssl::sha::Sha256::new();
    let mut file = File::open(path)?;
    let mut total = 0u64;
    loop {
        let mut chunk = [0; 65536];
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > 64 * 1024 * 1024 {
            return Err("Oversized story backup database.".into());
        }
        hash.update(&chunk[..n]);
    }
    Ok(hash.finish().iter().map(|b| format!("{b:02x}")).collect())
}
impl Rules {
    fn load(path: &Path) -> Result<Self> {
        let bytes = read_bounded(path, MAX_RULE_BYTES)?;
        let (catalog, settings) =
            nfs_services::progression::checkpoint_rules(&serde_json::from_slice(&bytes)?)
                .map_err(|_| "Invalid prepared checkpoint rules.")?;
        Ok(Self {
            catalog,
            settings,
            digest: digest(&bytes),
        })
    }
    fn policy(&self) -> Policy<'_> {
        Policy {
            catalog: &self.catalog,
            settings: self.settings,
        }
    }
}
impl Editor<'_> {
    fn repository(&self) -> Result<SqliteRepository> {
        Ok(SqliteRepository::open_existing_directory(
            &self.root.join("artifacts/account"),
        )?)
    }
    fn plan_id(&self, generation: u64, operation: u64, target: Checkpoint) -> Result<String> {
        Ok(digest(&serde_json::to_vec(
            &json!({"domain":"afterhours-story-plan-v1","account":self.account.hex(),
            "generation":generation.to_string(),"operation":operation.to_string(),"target":name(target),
            "rules":self.rules.digest,"supported":self.supported}),
        )?))
    }
    fn project(&self, snapshot: &Snapshot, target: Checkpoint, busy: bool) -> Result<Preview> {
        let policy = self.rules.policy();
        let state = policy.inspect(snapshot)?;
        if state.milestones.len() > 64 {
            return Err("The editable intro has too many boundaries.".into());
        }
        let position = match target {
            Checkpoint::AfterIntro => state.milestones.len(),
            Checkpoint::Objective(id) => state
                .milestones
                .iter()
                .position(|m| m.id == id)
                .ok_or("Unknown story checkpoint.")?,
        };
        let operation = policy.next_operation(snapshot)?;
        let milestone = |id| Milestone {
            id: format!("{id}"),
            label: self
                .rules
                .catalog
                .objective_label(id)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Objective {id}")),
        };
        let mut reasons = Vec::new();
        if !self.supported {
            reasons.push("This runtime cannot start from that checkpoint yet.".into());
        }
        if busy {
            reasons.push("Stop the game/server before changing story progress.".into());
        }
        Ok(Preview {
            version: 1,
            account_id: self.account.hex(),
            generation: snapshot.generation.to_string(),
            source_checkpoint: state
                .milestones
                .get(state.position)
                .map_or_else(|| "after-intro".into(), |m| m.id.to_string()),
            target_checkpoint: name(target),
            operation_id: operation.to_string(),
            plan_id: self.plan_id(snapshot.generation, operation, target)?,
            completes: state
                .milestones
                .iter()
                .take(position)
                .filter(|m| !m.completed)
                .map(|m| milestone(m.id))
                .collect(),
            reopens: state
                .milestones
                .iter()
                .skip(position)
                .filter(|m| m.completed)
                .map(|m| milestone(m.id))
                .collect(),
            can_apply: reasons.is_empty(),
            reasons,
        })
    }
    fn preview(&self, target: Checkpoint) -> Result<Preview> {
        let repo = self.repository()?;
        let snapshot = repo.inspect(self.account)?.snapshot;
        let busy = match session_lock(self.root, false) {
            Ok(_) => false,
            Err(e) if e.downcast_ref::<Error>() == Some(&Error::Busy) => true,
            Err(e) => return Err(e),
        } || repo.writer_is_leased(self.account)?;
        self.project(&snapshot, target, busy)
    }
    fn backup_root(&self, create: bool) -> Result<PathBuf> {
        let path = self.root.join("artifacts/story-backups");
        if create && !path.exists() {
            std::fs::create_dir(&path)?;
        }
        ordinary(&path)?;
        Ok(path)
    }
    fn verify_backup(&self, backup: &Backup, plan_id: &str) -> Result<()> {
        if !hex(&backup.id, 64) || !hex(&backup.database_sha256, 64) {
            return Err("Invalid story backup receipt.".into());
        }
        let path = self.backup_root(false)?.join(&backup.id);
        ordinary(&path)?;
        let manifest: serde_json::Value =
            serde_json::from_slice(&read_bounded(&path.join("manifest.json"), 4096)?)?;
        if manifest
            != json!({"version":1,"account_id":self.account.hex(),"generation":backup.generation,"plan_id":plan_id,
            "id":backup.id,"database_sha256":backup.database_sha256})
            || file_digest(&path.join(format!("{}.sqlite", self.account.hex())))?
                != backup.database_sha256
        {
            return Err("Story backup verification failed; no further edit was made.".into());
        }
        Ok(())
    }
    fn receipt(&self, snapshot: &Snapshot, proof: &Proof) -> Result<Option<(u64, Backup)>> {
        let Some(table) = snapshot.tables.get(&key("AfterhoursStoryEdits")) else {
            return Ok(None);
        };
        table.validate()?;
        let row = table.rows.get(&0).ok_or("Invalid story edit receipt.")?;
        if table.rows.len() != 1 || row.len() != 7 || row.get(&0) != Some(&Value::Int(1)) {
            return Err("Invalid story edit receipt.".into());
        }
        let text = |column| match row.get(&column) {
            Some(Value::String(s)) => Ok(s.as_str()),
            _ => Err("Invalid story edit receipt."),
        };
        let plan = text(1)?;
        let backup_id = text(2)?;
        let hash = text(3)?;
        let generation = decimal(text(4)?)?;
        let operation = decimal(text(5)?)?;
        let target = checkpoint(text(6)?)?;
        if !hex(plan, 64)
            || !hex(backup_id, 64)
            || !hex(hash, 64)
            || generation >= snapshot.generation
            || operation == 0
            || operation >= (1 << 60)
        {
            return Err("Invalid story edit receipt.".into());
        }
        if plan != proof.plan_id {
            return Ok(None);
        }
        if generation != proof.expected
            || target != proof.target
            || self.plan_id(generation, operation, target)? != plan
        {
            return Err("Story preview no longer matches these rules.".into());
        }
        Ok(Some((
            operation,
            Backup {
                id: backup_id.into(),
                generation: generation.to_string(),
                database_sha256: hash.into(),
            },
        )))
    }
    fn apply(&self, proof: &Proof, nonce: &str, now: Timestamp) -> Result<Applied> {
        if !self.supported {
            return Err("This runtime cannot start from that checkpoint yet.".into());
        }
        let _session = session_lock(self.root, true)?;
        let repo = self.repository()?;
        let lease = repo.writer_lease(self.account)?;
        let before = repo.inspect(self.account)?.snapshot;
        let policy = self.rules.policy();
        policy.inspect(&before)?;
        if let Some((operation, backup)) = self.receipt(&before, proof)? {
            if policy.jump(&before, proof.expected, operation, proof.target)? != Prepared::Replayed
            {
                return Err("Story receipt does not match the committed checkpoint.".into());
            }
            self.verify_backup(&backup, &proof.plan_id)?;
            return Ok(Applied {
                version: 1,
                account_id: self.account.hex(),
                previous_generation: proof.expected.to_string(),
                generation: before.generation.to_string(),
                target_checkpoint: name(proof.target),
                replayed: true,
                backup,
            });
        }
        if before.generation != proof.expected {
            return Err("The save changed; refresh the story preview.".into());
        }
        let preview = self.project(&before, proof.target, false)?;
        if preview.plan_id != proof.plan_id {
            return Err("The rules or checkpoint changed; refresh the story preview.".into());
        }
        let operation = decimal(&preview.operation_id)?;
        let Prepared::Mutation(mut batch) =
            policy.jump(&before, proof.expected, operation, proof.target)?
        else {
            return Err("Missing story edit receipt.".into());
        };
        let backup_id = digest(format!("{}:{nonce}", proof.plan_id).as_bytes());
        let directory = self.backup_root(true)?.join(&backup_id);
        std::fs::create_dir(&directory)?;
        let destination = SqliteRepository::open_existing_directory(&directory)?;
        repo.backup_into(&lease, proof.expected, &destination)?;
        let backup = Backup {
            id: backup_id,
            generation: proof.expected.to_string(),
            database_sha256: file_digest(&destination.path(self.account))?,
        };
        let manifest = json!({"version":1,"account_id":self.account.hex(),"generation":backup.generation,"plan_id":proof.plan_id,
            "id":backup.id,"database_sha256":backup.database_sha256});
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(directory.join("manifest.json"))?;
        file.write_all(&serde_json::to_vec(&manifest)?)?;
        file.sync_all()?;
        drop(file);
        self.verify_backup(&backup, &proof.plan_id)?;
        let receipt = Row::from([
            (0, Value::Int(1)),
            (1, Value::String(proof.plan_id.clone())),
            (2, Value::String(backup.id.clone())),
            (3, Value::String(backup.database_sha256.clone())),
            (4, Value::String(proof.expected.to_string())),
            (5, Value::String(operation.to_string())),
            (6, Value::String(name(proof.target))),
        ]);
        batch.ops.push(nfs_storage::Op::SetTable(
            key("AfterhoursStoryEdits"),
            Table {
                rows: BTreeMap::from([(0, receipt)]),
            },
        ));
        batch.validate()?;
        let applied = repo.apply(self.account, &batch, now)?;
        Ok(Applied {
            version: 1,
            account_id: self.account.hex(),
            previous_generation: proof.expected.to_string(),
            generation: applied.generation.to_string(),
            target_checkpoint: name(proof.target),
            replayed: applied.replayed,
            backup,
        })
    }
}

struct Args {
    root: PathBuf,
    account: AccountId,
    rules: PathBuf,
    target: Checkpoint,
    proof: Option<Proof>,
}
fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Args> {
    let mut fields = BTreeMap::new();
    while let Some(key) = args.next() {
        let value = args.next().ok_or("Missing story option value.")?;
        if fields.insert(key, value).is_some() {
            return Err("Repeated story option.".into());
        }
    }
    let mut take = |key: &str| {
        fields
            .remove(std::ffi::OsStr::new(key))
            .ok_or("Missing required story option.")
    };
    let root = PathBuf::from(take("--root")?);
    let rules = PathBuf::from(take("--progression-content")?);
    let account = take("--local-account")?;
    let account = account
        .to_str()
        .filter(|s| hex(s, 32))
        .ok_or("Invalid account identifier.")?;
    let mut bytes = [0; 16];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&account[i * 2..i * 2 + 2], 16)?;
    }
    let account = AccountId::from_owned_config(bytes)?;
    let action = match (
        fields.remove(std::ffi::OsStr::new("--preview")),
        fields.remove(std::ffi::OsStr::new("--apply")),
    ) {
        (Some(v), None) => (v, false),
        (None, Some(v)) => (v, true),
        _ => return Err("Choose exactly one of --preview or --apply.".into()),
    };
    let target = checkpoint(action.0.to_str().ok_or("Invalid checkpoint.")?)?;
    let proof = if action.1 {
        let expected = fields
            .remove(std::ffi::OsStr::new("--expected-generation"))
            .ok_or("Apply requires --expected-generation.")?;
        let plan = fields
            .remove(std::ffi::OsStr::new("--plan-id"))
            .ok_or("Apply requires --plan-id.")?;
        let plan = plan
            .to_str()
            .filter(|s| hex(s, 64))
            .ok_or("Invalid preview identifier.")?;
        Some(Proof {
            target,
            expected: decimal(expected.to_str().ok_or("Invalid generation.")?)?,
            plan_id: plan.into(),
        })
    } else {
        None
    };
    if !fields.is_empty() {
        return Err("Unknown or inapplicable story option.".into());
    }
    Ok(Args {
        root,
        account,
        rules,
        target,
        proof,
    })
}
fn run(args: Args) -> Result<serde_json::Value> {
    let root = root(&args.root)?;
    let rules = Rules::load(&args.rules)?;
    // No restart boundary is enabled before its prologue/runtime acceptance.
    // Tests inject supported constructed boundaries into Editor directly.
    let editor = Editor {
        root: &root,
        account: args.account,
        rules: &rules,
        supported: false,
    };
    match args.proof {
        None => Ok(serde_json::to_value(editor.preview(args.target)?)?),
        Some(proof) => {
            let clock = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
            let nonce = format!("{}:{}", std::process::id(), clock.as_nanos());
            let now = Timestamp(u64::try_from(clock.as_millis())?);
            Ok(serde_json::to_value(editor.apply(&proof, &nonce, now)?)?)
        }
    }
}
pub fn main(args: impl Iterator<Item = OsString>) -> std::process::ExitCode {
    let mut args = args.peekable();
    if args.peek().is_some_and(|s| s == "--help" || s == "-h") {
        println!("{HELP}");
        return std::process::ExitCode::SUCCESS;
    }
    match parse(args).and_then(run) {
        Ok(value) => {
            println!("{value}");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
