//! Monotonic controller leases. Every supervised write checks its token in the
//! same IMMEDIATE transaction as the mutation, including after a takeover.
use super::*;
use anyhow::ensure;

#[derive(Clone, Debug)]
pub(crate) struct LeaseToken {
    pub run_id: String,
    pub owner: String,
    epoch: i64,
}
impl LeaseToken {
    pub(crate) fn epoch(&self) -> i64 {
        self.epoch
    }
}

pub(super) fn check(tx: &Connection, token: &LeaseToken) -> Result<()> {
    let live: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM leases l JOIN lease_epochs e USING(run_id)
         WHERE l.run_id=?1 AND l.owner=?2 AND e.epoch=?3 AND l.expires_at>?4)",
        params![token.run_id, token.owner, token.epoch, UtcTimestamp::get()],
        |row| row.get(0),
    )?;
    ensure!(live, "controller lease lost; stale writes are fenced");
    Ok(())
}

impl Store {
    pub(crate) fn fenced(&self, token: LeaseToken) -> Self {
        Self {
            fence: Some(token),
            ..self.clone()
        }
    }

    pub(crate) fn claim_lease(
        &self,
        run: &str,
        owner: &str,
        seconds: i64,
    ) -> Result<Option<LeaseToken>> {
        ensure!(seconds > 0, "lease duration must be positive");
        self.transaction(|tx| {
            let now = UtcTimestamp::get();
            let previous: Option<(String, i64)> = tx.query_row(
                "SELECT owner,expires_at FROM leases WHERE run_id=?1", [run],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            if previous.as_ref().is_some_and(|(held, expiry)| held != owner && *expiry > now) {
                return Ok(None);
            }
            if previous.as_ref().is_none_or(|(held, expiry)| held != owner || *expiry <= now) {
                tx.execute("INSERT INTO lease_epochs VALUES(?1,1) ON CONFLICT(run_id) DO UPDATE SET epoch=epoch+1", [run])?;
            } else {
                tx.execute("INSERT OR IGNORE INTO lease_epochs VALUES(?1,1)", [run])?;
            }
            let epoch = tx.query_row("SELECT epoch FROM lease_epochs WHERE run_id=?1", [run], |row| row.get(0))?;
            tx.execute("INSERT INTO leases VALUES(?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET owner=excluded.owner,expires_at=excluded.expires_at", params![run,owner,now+seconds])?;
            Ok(Some(LeaseToken { run_id:run.into(), owner:owner.into(), epoch }))
        })
    }

    pub(crate) fn renew_lease(&self, token: &LeaseToken, seconds: i64) -> Result<()> {
        self.transaction(|tx| {
            check(tx, token)?;
            tx.execute(
                "UPDATE leases SET expires_at=?1 WHERE run_id=?2",
                params![UtcTimestamp::get() + seconds, token.run_id],
            )?;
            Ok(())
        })
    }

    pub(crate) fn release_token(&self, token: &LeaseToken) -> Result<()> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM leases WHERE run_id=?1 AND owner=?2 AND EXISTS(SELECT 1 FROM lease_epochs WHERE run_id=?1 AND epoch=?3)", params![token.run_id,token.owner,token.epoch])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_controller_cannot_write_renew_or_release_a_new_lease() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path())?;
        let first = store.claim_lease("run", "owner", 20)?.unwrap();
        let stale = store.fenced(first.clone());
        stale.put("fixture", "id", "run", &json!("before"))?;
        store.transaction(|tx| {
            tx.execute("UPDATE leases SET expires_at=0", [])?;
            Ok(())
        })?;
        assert!(store.renew_lease(&first, 20).is_err());
        let next = store.claim_lease("run", "owner", 20)?.unwrap();
        assert!(next.epoch > first.epoch);
        assert!(stale.put("fixture", "id", "run", &json!("stale")).is_err());
        store.release_token(&first)?;
        store.renew_lease(&next, 20)?;
        assert_eq!(store.require::<Value>("fixture", "id")?, json!("before"));
        store
            .fenced(next)
            .put("fixture", "id", "run", &json!("after"))?;
        Ok(())
    }
}
