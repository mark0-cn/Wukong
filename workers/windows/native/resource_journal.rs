//! System-owned resource/effect state, using SQLite transactions rather than a
//! second filesystem transaction engine. This is not an execution broker API.
pub use crate::{inputs::Owner, lease_proxy::Identity as Binding};
use crate::{
    inputs::digest,
    security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    time::Duration,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetFileInformationByHandle,
};

const APPLICATION_ID: i64 = 0x574b4e52;
const MAX_JSON: usize = 64 * 1024;
const MAX_EFFECTS: i64 = 100_000;
const SCHEMA: &str = "
CREATE TABLE runs(run_id TEXT PRIMARY KEY, binding TEXT NOT NULL) STRICT;
CREATE TABLE resources(id TEXT PRIMARY KEY CHECK(length(id)=32), run_id TEXT NOT NULL REFERENCES runs(run_id),
    phase TEXT NOT NULL CHECK(phase IN ('preparing','live','closing','closed'))) STRICT;
CREATE UNIQUE INDEX one_active_resource ON resources((1)) WHERE phase <> 'closed';
CREATE TABLE effects(id TEXT PRIMARY KEY CHECK(length(id)=64), resource TEXT NOT NULL REFERENCES resources(id),
    fingerprint TEXT NOT NULL CHECK(length(fingerprint)=64), payload TEXT NOT NULL,
    actor TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('reserved','started','completed')),
    ticket TEXT, outcome TEXT,
    CHECK((state='completed')=(outcome IS NOT NULL)),
    CHECK(state<>'started' OR ticket IS NOT NULL)) STRICT;
CREATE UNIQUE INDEX one_pending_mutation ON effects(resource) WHERE state <> 'completed';
";

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Process,
    Debugger,
    Replay,
    Interactive,
    Python,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Live,
    Closing,
    Closed,
}
impl Phase {
    fn read(value: &str) -> Result<Self> {
        match value {
            "preparing" => Ok(Self::Preparing),
            "live" => Ok(Self::Live),
            "closing" => Ok(Self::Closing),
            "closed" => Ok(Self::Closed),
            _ => anyhow::bail!("invalid native resource phase"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Result {
        value: Value,
    },
    /// Cleanup establishes quiescence, not what an interrupted mutation did.
    UncertainClosed,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Reservation {
    Reserved {
        effect: String,
        resource: String,
        phase: Phase,
    },
    Pending {
        effect: String,
        resource: String,
        phase: Phase,
    },
    Completed {
        effect: String,
        resource: String,
        phase: Phase,
        outcome: Outcome,
    },
}

/// Only begin() issues a ticket. It is deliberately not a wire-deserializable
/// authority, and another begin cannot replay a started effect.
pub struct Ticket {
    effect: String,
    resource: String,
    nonce: String,
    incarnation: String,
}

/// Business preconditions supplied by the owning native runtime, never accepted
/// from a worker RPC. These flags alone are not independent OS evidence.
pub struct Cleanup {
    pub jobs_empty: bool,
    pub profiles_removed: bool,
    pub scratch_removed: bool,
}

pub struct Journal {
    connection: Connection,
    binding: Binding,
    incarnation: String,
    // Drop SQLite before the no-delete database reservation and writer lock.
    _database: fs::File,
    _writer: fs::File,
}
impl Journal {
    pub fn open(metadata: &Path, binding: Binding) -> Result<Self> {
        security::require_system()?;
        Self::with_policy(metadata, binding, AclPolicy::private_vm())
    }
    pub(crate) fn with_policy(
        metadata: &Path,
        binding: Binding,
        policy: AclPolicy,
    ) -> Result<Self> {
        Binding::from_readiness(
            serde_json::to_value(&binding)?,
            &binding.installation_id,
            &binding.package_sha256,
        )?;
        crate::pipe_transport::service_name(&binding.installation_id)?;
        policy.verify(metadata, true)?;
        let root = metadata.canonicalize()?.join("native-resources");
        if !root.exists() {
            policy.create_directory(&root)?;
        }
        policy.verify(&root, true)?;
        for entry in fs::read_dir(&root)? {
            let entry = entry?;
            ensure!(
                matches!(
                    entry.file_name().to_str(),
                    Some("writer.lock" | "resources.sqlite" | "resources.sqlite-journal")
                ),
                "unrecognized native resource database member"
            );
            security::no_links(&entry.path())?;
        }
        let lock = root.join("writer.lock");
        if !lock.exists() {
            policy.create_file(&lock)?.sync_all()?;
        }
        policy.verify(&lock, false)?;
        let writer = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&lock)?;
        regular(&writer, 0)?;
        let database = root.join("resources.sqlite");
        if !database.exists() {
            policy.create_file(&database)?.sync_all()?;
        }
        policy.verify(&database, false)?;
        // Data access establishes the sharing reservation; an attributes-only
        // handle would not prevent database replacement on Windows.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&database)?;
        regular(&held, 64 * 1024 * 1024)?;
        // A killed writer's rollback journal inherits the private directory's
        // grants. Seal only this known, single-link sidecar before recovery;
        // SQLite must remain able to delete it after restoring the database.
        let rollback = root.join("resources.sqlite-journal");
        if rollback.exists() {
            let file = fs::OpenOptions::new()
                .read(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&rollback)?;
            regular(&file, 128 * 1024 * 1024)?;
            drop(file);
            policy.apply(&rollback, false)?;
        }
        let mut connection = Connection::open_with_flags(
            &database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_FULL_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_millis(100))?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA journal_mode=DELETE; PRAGMA temp_store=MEMORY; PRAGMA max_page_count=16384;")?;
        ensure!(
            connection.pragma_query_value(None, "page_size", |row| row.get::<_, i64>(0))? == 4096,
            "native resource database page size differs"
        );
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version == 0 {
            ensure!(
                connection
                    .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))?
                    == 0,
                "unrecognized native resource database application"
            );
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |row| row.get(0),
            )?;
            ensure!(tables == 0, "unrecognized native resource database");
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute_batch(SCHEMA)?;
            transaction.pragma_update(None, "application_id", APPLICATION_ID)?;
            transaction.pragma_update(None, "user_version", 1)?;
            transaction.commit()?;
        }
        ensure!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? == 1
                && connection
                    .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))?
                    == APPLICATION_ID,
            "unsupported native resource database identity"
        );
        Ok(Self {
            connection,
            binding,
            incarnation: hex::encode(crate::accounts::random_bytes::<16>()?),
            _database: held,
            _writer: writer,
        })
    }
    fn owner(&self, owner: &Owner) -> Result<()> {
        ensure!(
            owner.identity == self.binding && owner.epoch > 0,
            "native resource owner binding differs"
        );
        bounded_identifier(&owner.run_id, 128)?;
        bounded_identifier(&owner.owner_id, 128)
    }
    pub fn idle(&self) -> Result<bool> {
        Ok(self.connection.query_row("SELECT NOT EXISTS(SELECT 1 FROM resources WHERE phase<>'closed') AND NOT EXISTS(SELECT 1 FROM effects WHERE state<>'completed')",[],|row|row.get(0))?)
    }
    pub fn reserve_start(
        &mut self,
        owner: &Owner,
        request: &str,
        kind: ResourceKind,
        specification: &Value,
    ) -> Result<Reservation> {
        self.owner(owner)?;
        bounded_identifier(request, 256)?;
        let payload = bounded_json(
            &serde_json::json!({"operation":"start","kind":kind,"specification":specification}),
        )?;
        let effect = effect_id(owner, request)?;
        let fingerprint = digest(&("native.resource.specification.v1", &payload))?;
        let binding = serde_json::to_string(&owner.identity)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_run(&transaction, &owner.run_id, &binding)?;
        if let Some(existing) = existing(&transaction, &effect, &fingerprint)? {
            return Ok(existing);
        }
        ensure!(
            transaction.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM resources WHERE phase<>'closed')",
                [],
                |row| row.get::<_, bool>(0)
            )?,
            "another native resource remains active or uncertain"
        );
        ensure!(
            transaction.query_row("SELECT count(*) FROM effects", [], |row| row
                .get::<_, i64>(0))?
                < MAX_EFFECTS,
            "native resource effect budget exhausted"
        );
        let resource = hex::encode(crate::accounts::random_bytes::<16>()?);
        transaction.execute(
            "INSERT OR IGNORE INTO runs VALUES(?1,?2)",
            params![owner.run_id, binding],
        )?;
        transaction.execute(
            "INSERT INTO resources VALUES(?1,?2,'preparing')",
            params![resource, owner.run_id],
        )?;
        transaction.execute(
            "INSERT INTO effects VALUES(?1,?2,?3,?4,?5,'reserved',NULL,NULL)",
            params![effect, resource, fingerprint, payload, bounded_json(owner)?],
        )?;
        transaction.commit()?;
        Ok(Reservation::Reserved {
            effect,
            resource,
            phase: Phase::Preparing,
        })
    }
    pub fn begin(&mut self, effect: &str) -> Result<Ticket> {
        let nonce = hex::encode(crate::accounts::random_bytes::<16>()?);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (resource, actor): (String,String) = transaction.query_row("SELECT e.resource,e.actor FROM effects e JOIN resources r ON r.id=e.resource WHERE e.id=?1 AND e.state='reserved' AND r.phase='preparing'",[effect],|row|Ok((row.get(0)?,row.get(1)?)))
            .context("native effect already started or settled; reconcile instead of replaying")?;
        ensure!(
            serde_json::from_str::<Owner>(&actor)?.identity == self.binding,
            "native effect belongs to a previous worker binding"
        );
        let changed = transaction.execute(
            "UPDATE effects SET state='started',ticket=?2 WHERE id=?1 AND state='reserved'",
            params![effect, nonce],
        )?;
        ensure!(changed == 1, "native effect reservation changed");
        transaction.commit()?;
        Ok(Ticket {
            effect: effect.into(),
            resource,
            nonce,
            incarnation: self.incarnation.clone(),
        })
    }
    pub fn complete(&mut self, ticket: &Ticket, outcome: &Outcome) -> Result<()> {
        ensure!(
            matches!(outcome, Outcome::Result { .. }),
            "uncertain outcomes require verified cleanup"
        );
        ensure!(
            ticket.incarnation == self.incarnation,
            "native completion belongs to a previous journal incarnation"
        );
        let encoded = bounded_json(outcome)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: (String, Option<String>) = transaction.query_row(
            "SELECT state,outcome FROM effects WHERE id=?1 AND resource=?2 AND ticket=?3",
            params![ticket.effect, ticket.resource, ticket.nonce],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if current.0 == "completed" {
            ensure!(
                current.1.as_deref() == Some(&encoded),
                "native completed effect cannot be replaced"
            );
            return Ok(());
        }
        ensure!(
            current.0 == "started",
            "native effect is not awaiting this outcome"
        );
        transaction.execute(
            "UPDATE effects SET state='completed',outcome=?2 WHERE id=?1",
            params![ticket.effect, encoded],
        )?;
        transaction.execute(
            "UPDATE resources SET phase='live' WHERE id=?1 AND phase='preparing'",
            [&ticket.resource],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn start_closing(&mut self, resource: &str) -> Result<()> {
        ensure!(
            self.connection.execute(
                "UPDATE resources SET phase='closing' WHERE id=?1 AND phase<>'closed'",
                [resource]
            )? == 1,
            "native resource is absent or closed"
        );
        Ok(())
    }
    pub fn close(&mut self, resource: &str, cleanup: Cleanup) -> Result<()> {
        ensure!(
            cleanup.jobs_empty && cleanup.profiles_removed && cleanup.scratch_removed,
            "native resource cleanup is not quiescent and complete"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let phase: String = transaction.query_row(
            "SELECT phase FROM resources WHERE id=?1",
            [resource],
            |row| row.get(0),
        )?;
        ensure!(
            phase == "closing",
            "native resource must enter closing before release"
        );
        // Never manufacture success for a mutation whose reply was lost.
        transaction.execute("UPDATE effects SET state='completed',outcome=?2 WHERE resource=?1 AND state<>'completed'",
            params![resource,bounded_json(&Outcome::UncertainClosed)?])?;
        transaction.execute(
            "UPDATE resources SET phase='closed' WHERE id=?1",
            [resource],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn regular(file: &fs::File, maximum: u64) -> Result<()> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    security::check(unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) })?;
    ensure!(
        info.nNumberOfLinks == 1
            && info.dwFileAttributes & 0x400 == 0
            && file.metadata()?.len() <= maximum,
        "native resource database member is linked, redirected or oversized"
    );
    Ok(())
}
fn bounded_identifier(value: &str, maximum: usize) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control),
        "invalid native resource identifier"
    );
    Ok(())
}
fn bounded_json(value: &impl Serialize) -> Result<String> {
    let value = serde_json::to_string(value)?;
    ensure!(
        value.len() <= MAX_JSON,
        "native resource record exceeds 64 KiB"
    );
    Ok(value)
}
fn effect_id(owner: &Owner, request: &str) -> Result<String> {
    // Controller incarnation/lease epoch are audit data, not business identity.
    digest(&(
        "native.resource.effect.v1",
        &owner.identity,
        &owner.run_id,
        request,
    ))
}
fn check_run(transaction: &Transaction<'_>, run: &str, binding: &str) -> Result<()> {
    let previous: Option<String> = transaction
        .query_row("SELECT binding FROM runs WHERE run_id=?1", [run], |row| {
            row.get(0)
        })
        .optional()?;
    ensure!(
        previous
            .as_deref()
            .is_none_or(|previous| previous == binding),
        "native run binding changed"
    );
    Ok(())
}
fn existing(
    transaction: &Transaction<'_>,
    effect: &str,
    fingerprint: &str,
) -> Result<Option<Reservation>> {
    let row: Option<(String,String,String,String,Option<String>)> = transaction.query_row(
        "SELECT e.fingerprint,e.resource,e.state,r.phase,e.outcome FROM effects e JOIN resources r ON r.id=e.resource WHERE e.id=?1",[effect],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    let Some((previous, resource, state, phase, outcome)) = row else {
        return Ok(None);
    };
    ensure!(
        previous == fingerprint,
        "native request_id already identifies different resource parameters"
    );
    let phase = Phase::read(&phase)?;
    Ok(Some(match state.as_str() {
        "reserved" => Reservation::Reserved {
            effect: effect.into(),
            resource,
            phase,
        },
        "started" => Reservation::Pending {
            effect: effect.into(),
            resource,
            phase,
        },
        "completed" => Reservation::Completed {
            effect: effect.into(),
            resource,
            phase,
            outcome: serde_json::from_str(
                &outcome.context("native completed effect lacks an outcome")?,
            )?,
        },
        _ => anyhow::bail!("invalid native effect state"),
    }))
}

#[cfg(test)]
#[path = "resource_journal_tests.rs"]
mod tests;
