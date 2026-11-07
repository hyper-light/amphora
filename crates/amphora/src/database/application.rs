//! Retained application writer boundary on the existing standalone owner (U05; replication.md,
//! "The application floor, replicated", "Today"). The database lease excludes another engine,
//! while this floor also fences old attached application writers at the actual serialized
//! transaction boundary.

use super::*;
use serde::{Deserialize, Serialize};

// Created by the first raise, inside its own session, never with the product schema: a replica
// opens that schema locally, and a member on a binary from before the floor would not create this
// table, so its pages would differ (replication.md, "The upgrade that introduces the join rows").
const FLOOR: &str = "CREATE TABLE IF NOT EXISTS __amphora_application_floor (singleton INTEGER PRIMARY KEY CHECK(singleton=1), application TEXT NOT NULL, version INTEGER NOT NULL CHECK(version>0))";

// An application's name is a family-shaped identifier; this bounds it as admission policy, not
// a measured size.
const NAME_BYTES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Application {
    pub(crate) name: String,
    pub(crate) version: u32,
}
impl Application {
    pub(crate) fn validate(&self) -> Result<(), Refusal> {
        api::family(&self.name)?;
        api::bound("application identity", self.name.len(), NAME_BYTES)?;
        if self.version == 0 {
            return Err(api::invalid("application contract", "zero version"));
        }
        Ok(())
    }
}

/// An actual successful floor barrier, not deserialized metadata or an attachment
/// claiming it holds the original owner. Keeping this value retains that owner,
/// its engine and its original lease through the caller's maintenance lifetime.
pub struct WriterBoundary {
    database: Database,
    floor: Application,
}
impl WriterBoundary {
    /// Identity only. The same owner's writer rechecks the persisted floor inside
    /// every application transaction, including queued pre-floor requests.
    pub fn require(&self, database: &Database) -> Result<(), Refusal> {
        if !Arc::ptr_eq(&self.database.shared, &database.shared)
            || database.application.as_ref() != self.database.application.as_ref()
        {
            return Err(api::invalid(
                "writer boundary",
                "different original owner or application",
            ));
        }
        Ok(())
    }
    pub fn version(&self) -> u32 {
        self.floor.version
    }
}

impl Database {
    /// Identity comparison only; this does not establish a writer floor.
    pub fn require_same_application(&self, other: &Self) -> Result<(), Refusal> {
        if !Arc::ptr_eq(&self.shared, &other.shared) || self.application != other.application {
            return Err(api::invalid(
                "application database identity",
                "different retained database or application",
            ));
        }
        Ok(())
    }
    /// Bind a handle and all namespaces it creates to this application's format.
    /// No floor is installed and no authority is created by this declaration.
    pub fn for_application(&self, name: &str, version: u32) -> Result<Self, Refusal> {
        let application = Application {
            name: name.into(),
            version,
        };
        application.validate()?;
        // A replica's owner keeps this member's attestation of it (replication.md, "Attestation").
        if let Writer::Replicated { client, .. } = &self.shared.writer {
            client.handle().attest(application.clone())?;
        }
        let mut selected = self.clone();
        selected.application = Some(application);
        Ok(selected)
    }

    /// Install/observe the monotonic contract in the SAME original owner: its local writer, or
    /// its replica, where the raise is proven in its own session by every member of the
    /// configuration applied at its base (replication.md, "Raising the floor"). Return only after
    /// its normal durable callback. A failed/unknown callback yields no boundary. Remote-owner
    /// takeover needs its own application-floor proof and is refused here.
    pub fn writer_floor(
        &self,
        name: &str,
        required: u32,
        until: Instant,
    ) -> Result<WriterBoundary, Refusal> {
        let replicated = matches!(&self.shared.writer, Writer::Replicated { .. });
        if !(matches!(&self.shared.writer, Writer::Local(_)) || replicated)
            || self.shared.restoring.load(Ordering::Acquire)
        {
            return Err(api::invalid(
                "writer boundary",
                "original non-restoring owner required",
            ));
        }
        let database = self.clone();
        let application = database
            .application
            .clone()
            .ok_or_else(|| api::invalid("writer boundary", "application-bound handle required"))?;
        application.validate()?;
        if application.name != name || required == 0 || required > application.version {
            return Err(api::invalid(
                "writer boundary",
                "requested contract exceeds this application",
            ));
        }
        let wanted = Application {
            name: name.into(),
            version: required,
        };
        let floor = database
            .call_db(until, move |db| raise(db, wanted, replicated))
            .map_err(|refusal| {
                if replicated {
                    overtaken(refusal)
                } else {
                    refusal
                }
            })?;
        Ok(WriterBoundary { database, floor })
    }
}

/// The raise's session: the floor becomes `wanted`'s version, or stays above it, for `wanted`'s
/// application; on a replica only on the members' proof (replication.md, "Raising the floor").
pub(super) fn raise(
    db: &mut Db,
    wanted: Application,
    replicated: bool,
) -> anyhow::Result<Application> {
    let current = policy(db)?;
    if let Some(current) = &current
        && current.name != wanted.name
    {
        return Err(
            Refusal::Fenced("another application's writer floor is installed".into()).into(),
        );
    }
    // The floor never falls: a raise to it or below answers the floor in force at once, with
    // nothing to prove and nothing written (a caller asking for an older contract is served by
    // the newer one in force, as kiln's writer_boundary asks).
    if let Some(current) = current
        && current.version >= wanted.version
    {
        return Ok(current);
    }
    let version = wanted.version;
    if replicated {
        prove(db, &wanted.name, version)?;
    }
    db.exec(FLOOR)?;
    db.query(
        "INSERT INTO __amphora_application_floor(singleton, application, version) \
             VALUES(1, ?1, ?2) ON CONFLICT(singleton) DO UPDATE SET version=excluded.version",
        [text(&wanted.name), EValue::from_i64(i64::from(version))],
        |_| Ok(true),
    )?;
    Ok(Application {
        name: wanted.name,
        version,
    })
}

/// What a replicated raise answers when the stale-base rule refuses it: the group changed
/// between its proof and its index, so it is to be asked again, `Unavailable`, the class for
/// "retry later" (amphora-seam-owner's ruling on step 4, a). Every other answer is its own: a
/// displaced entry stays UnknownOutcome.
pub(super) fn overtaken(refusal: Refusal) -> Refusal {
    match refusal {
        Refusal::Invalid { what, why }
            if what == "prepared transaction"
                && why == "the database advanced since preparation" =>
        {
            Refusal::Unavailable("the database advanced since preparation: the group changed while the raise was proven; raise again".into())
        }
        other => other,
    }
}

/// The members' proof of a raise to `version` on a replica, read at this session's base; the
/// stale-base rule makes it hold where it applies (replication.md, "Raising the floor").
fn prove(db: &Db, application: &str, version: u32) -> anyhow::Result<()> {
    use crate::replication::floor;
    if floor::tracked(db)?.is_none() {
        return Err(Refusal::Unavailable(
            "joins aren't tracked yet; every member must attest first".into(),
        )
        .into());
    }
    if !floor::configuration(db)?.voters_outgoing.is_empty() {
        return Err(Refusal::Unavailable(
            "the group is leaving a joint configuration; raise the floor once it has".into(),
        )
        .into());
    }
    let short = floor::short_of(db, application, version)?;
    if !short.is_empty() {
        return Err(Refusal::Unavailable(format!(
            "members {short:?} have not attested {application} at version {version} or above"
        ))
        .into());
    }
    Ok(())
}

/// The floor in force at this session's base, if one is installed.
pub(super) fn policy(db: &Db) -> anyhow::Result<Option<Application>> {
    let mut present = false;
    db.query(
        "SELECT 1 FROM sqlite_schema WHERE type='table' AND name='__amphora_application_floor'",
        [],
        |_| {
            present = true;
            Ok(false)
        },
    )?;
    if !present {
        return Ok(None);
    }
    let mut value = None;
    db.query(
        "SELECT application, version FROM __amphora_application_floor WHERE singleton=1",
        [],
        |row| {
            anyhow::ensure!(value.is_none(), "duplicate application writer floor");
            value = Some(Application {
                name: row.get::<String>(0).map_err(unavailable)?,
                version: u32::try_from(row.get::<i64>(1).map_err(unavailable)?)?,
            });
            Ok(true)
        },
    )?;
    if let Some(value) = &value {
        value.validate()?;
    }
    Ok(value)
}

pub(super) fn check(
    db: &Db,
    claim: Option<&Application>,
    operation: &Operation,
) -> anyhow::Result<()> {
    // The owner's initial durability barrier has no SQL or product effect and
    // must run before open returns an application-bindable handle.
    if matches!(operation, Operation::Mutation(Mutation::Sql(statements)) if statements.is_empty())
    {
        return Ok(());
    }
    claim_holds(db, claim)
}

/// Whether `claim` holds against the floor installed in `db`: reads and writes alike, in every
/// mode, so a handle's answer doesn't depend on where its database runs (CLAUDE.md section 8).
pub(super) fn claim_holds(db: &Db, claim: Option<&Application>) -> anyhow::Result<()> {
    let Some(floor) = policy(db)? else {
        return Ok(());
    };
    let claim =
        claim.ok_or_else(|| Refusal::Fenced("application writer contract required".into()))?;
    claim.validate()?;
    if claim.name != floor.name {
        return Err(Refusal::Fenced(
            "application writer contract is for another application than the installed floor"
                .into(),
        )
        .into());
    }
    if claim.version < floor.version {
        return Err(Refusal::Fenced(
            "application writer contract is older than the installed floor".into(),
        )
        .into());
    }
    Ok(())
}
