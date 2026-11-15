//! System-owned resource/effect state, using SQLite transactions rather than a
//! second filesystem transaction engine. This is not an execution broker API.
use crate::artifacts::inputs::Owner;
use crate::resources::models::bounded_identifier;
use crate::resources::models::{MutationKind, Phase, ResourceKind, ResourceRecord};
use crate::{
    artifacts::inputs::digest,
    isolation::security::{self, AclPolicy},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
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
use wukong_worker_protocol::WorkerIdentity as Binding;

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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartPayload {
    operation: String,
    kind: ResourceKind,
    specification: Value,
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
    pub(crate) fn with_policy(
        metadata: &Path,
        binding: Binding,
        policy: AclPolicy,
    ) -> Result<Self> {
        crate::rpc::worker_identity::validate(&binding)?;
        crate::rpc::pipe_transport::service_name(&binding.installation_id)?;
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
            transaction.pragma_update(None, "user_version", 2)?;
            transaction.commit()?;
        }
        ensure!(
            connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))? == 2
                && connection
                    .pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0))?
                    == APPLICATION_ID,
            "unsupported native resource database identity"
        );
        connection.execute_batch("CREATE INDEX IF NOT EXISTS resources_by_run ON resources(run_id,id); CREATE INDEX IF NOT EXISTS effects_by_resource ON effects(resource);")?;
        Ok(Self {
            connection,
            binding,
            incarnation: hex::encode(crate::host::accounts::random_bytes::<16>()?),
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
    /// System recovery can observe old bindings, but must reconcile their OS
    /// resources before close(); observation never reopens or authorizes work.
    pub fn active(&self) -> Result<Option<ResourceRecord>> {
        let record = resource_record(&self.connection, None)?;
        ensure!(
            record.is_some() || self.idle()?,
            "native resource state lacks a recoverable start intent"
        );
        Ok(record)
    }
    pub fn resource(&self, owner: &Owner, id: &str) -> Result<ResourceRecord> {
        self.owner(owner)?;
        let record =
            resource_record(&self.connection, Some(id))?.context("native resource is absent")?;
        ensure!(
            record.owner.identity == owner.identity && record.owner.run_id == owner.run_id,
            "native resource belongs to another worker binding or run"
        );
        Ok(record)
    }
    /// A bounded, run-scoped discovery view. Every returned intent still goes
    /// through the same fingerprint/owner checks as a direct resource lookup.
    pub fn resources(
        &self,
        owner: &Owner,
        kind: Option<ResourceKind>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ResourceRecord>> {
        self.owner(owner)?;
        check_run(
            &self.connection,
            &owner.run_id,
            &serde_json::to_string(&owner.identity)?,
        )?;
        ensure!(
            (1..=64).contains(&limit),
            "native inventory limit must be 1..64"
        );
        if let Some(after) = after {
            ensure!(
                after.len() == 32
                    && after
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                "native inventory cursor must be a canonical resource ID"
            );
        }
        let encoded_kind = kind.map(serde_json::to_value).transpose()?;
        let name = encoded_kind.as_ref().and_then(Value::as_str);
        let mut statement = self.connection.prepare(
            "SELECT r.id FROM resources r WHERE r.run_id=?1 AND (?2 IS NULL OR r.id>?2)
             AND (?3 IS NULL OR EXISTS(SELECT 1 FROM effects e WHERE e.resource=r.id
                 AND json_extract(e.payload,'$.operation')='start' AND json_extract(e.payload,'$.kind')=?3))
             ORDER BY r.id LIMIT ?4",
        )?;
        let ids = statement
            .query_map(params![owner.run_id, after, name, limit], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.into_iter()
            .map(|id| {
                let record = self.resource(owner, &id)?;
                ensure!(
                    kind.is_none_or(|kind| record.kind == kind),
                    "native inventory kind differs from its original intent"
                );
                Ok(record)
            })
            .collect()
    }
    pub fn reserve_mutation(
        &mut self,
        owner: &Owner,
        request: &str,
        resource: &str,
        kind: MutationKind,
        specification: &Value,
    ) -> Result<Reservation> {
        let record = self.resource(owner, resource)?;
        ensure!(
            matches!(
                (record.kind, kind),
                (ResourceKind::Debugger, MutationKind::DebugControl)
                    | (ResourceKind::Interactive, MutationKind::InteractiveWrite)
            ),
            "native mutation does not match the resource kind"
        );
        bounded_identifier(request, 256)?;
        let payload = bounded_json(
            &serde_json::json!({"operation":"mutation","resource":resource,"kind":kind,"specification":specification}),
        )?;
        let effect = effect_id(owner, request)?;
        let fingerprint = digest(&("native.resource.specification.v1", &payload))?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_run(
            &transaction,
            &owner.run_id,
            &serde_json::to_string(&owner.identity)?,
        )?;
        if let Some(existing) = existing(&transaction, &effect, &fingerprint)? {
            return Ok(existing);
        }
        ensure!(
            record.phase == Phase::Live,
            "native resource is not live for a new mutation"
        );
        ensure!(
            transaction.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM effects WHERE resource=?1 AND state<>'completed')",
                [resource],
                |row| row.get::<_, bool>(0)
            )?,
            "native resource already has an active or uncertain mutation"
        );
        ensure!(
            transaction.query_row("SELECT count(*) FROM effects", [], |row| row
                .get::<_, i64>(0))?
                < MAX_EFFECTS,
            "native resource effect budget exhausted"
        );
        transaction.execute(
            "INSERT INTO effects VALUES(?1,?2,?3,?4,?5,'reserved',NULL,NULL)",
            params![effect, resource, fingerprint, payload, bounded_json(owner)?],
        )?;
        transaction.commit()?;
        Ok(Reservation::Reserved {
            effect,
            resource: resource.into(),
            phase: Phase::Live,
        })
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
        let resource = hex::encode(crate::host::accounts::random_bytes::<16>()?);
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
    /// Read a stable start effect before resolving inputs under a renewed
    /// controller fence. A cached receipt never grants permission to relaunch.
    pub fn lookup_start(
        &self,
        owner: &Owner,
        request: &str,
        kind: ResourceKind,
        specification: &Value,
    ) -> Result<Option<Reservation>> {
        self.owner(owner)?;
        bounded_identifier(request, 256)?;
        let payload = bounded_json(
            &serde_json::json!({"operation":"start","kind":kind,"specification":specification}),
        )?;
        existing(
            &self.connection,
            &effect_id(owner, request)?,
            &digest(&("native.resource.specification.v1", &payload))?,
        )
    }
    pub fn begin(&mut self, effect: &str) -> Result<Ticket> {
        let nonce = hex::encode(crate::host::accounts::random_bytes::<16>()?);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (resource, actor): (String,String) = transaction.query_row("SELECT e.resource,e.actor FROM effects e JOIN resources r ON r.id=e.resource WHERE e.id=?1 AND e.state='reserved' AND r.phase IN ('preparing','live')",[effect],|row|Ok((row.get(0)?,row.get(1)?)))
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

fn resource_record(connection: &Connection, id: Option<&str>) -> Result<Option<ResourceRecord>> {
    struct StoredStart {
        id: String,
        phase: String,
        run: String,
        start_effect: String,
        actor: String,
        payload: String,
        fingerprint: String,
        count: i64,
    }
    let row: Option<StoredStart> = connection.query_row(
        "SELECT r.id,r.phase,r.run_id,e.id,e.actor,e.payload,e.fingerprint,count(*) OVER () FROM resources r JOIN effects e ON e.resource=r.id
         WHERE ((?1 IS NULL AND r.phase<>'closed') OR r.id=?1) AND json_extract(e.payload,'$.operation')='start'",
        [id],|row|Ok(StoredStart{id:row.get(0)?,phase:row.get(1)?,run:row.get(2)?,start_effect:row.get(3)?,actor:row.get(4)?,payload:row.get(5)?,fingerprint:row.get(6)?,count:row.get(7)?})).optional()?;
    let Some(StoredStart {
        id,
        phase,
        run,
        start_effect,
        actor,
        payload,
        fingerprint,
        count,
    }) = row
    else {
        return Ok(None);
    };
    ensure!(count == 1, "native resource has ambiguous start intents");
    ensure!(
        actor.len() <= MAX_JSON && payload.len() <= MAX_JSON,
        "native resource intent exceeds record bounds"
    );
    ensure!(
        digest(&("native.resource.specification.v1", &payload))? == fingerprint,
        "native resource start fingerprint differs"
    );
    let owner: Owner = serde_json::from_str(&actor)?;
    let payload: StartPayload = serde_json::from_str(&payload)?;
    ensure!(
        owner.run_id == run && payload.operation == "start",
        "native resource intent scope differs"
    );
    Ok(Some(ResourceRecord {
        id,
        phase: Phase::read(&phase)?,
        start_effect,
        owner,
        kind: payload.kind,
        specification: payload.specification,
    }))
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
fn check_run(connection: &Connection, run: &str, binding: &str) -> Result<()> {
    let previous: Option<String> = connection
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
    transaction: &Connection,
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
#[path = "journal_tests.rs"]
mod tests;
