//! SDK attachment to the canonical owner. No database or Raft actor is created for clients.
use super::*;
use serde::{Deserialize, Serialize};

pub(super) struct Backend {
    pub(super) client: crate::transport::Client,
    path: PathBuf,
    restart: Mutex<Option<(PathBuf, api::replication::ReplicaConfig)>>,
    promoted: Mutex<Option<Database>>,
}
impl Backend {
    pub(super) fn configure(
        &self,
        replica: Option<(&Path, &api::replication::ReplicaConfig)>,
    ) -> Result<(), Refusal> {
        if let Some((path, config)) = replica {
            let mut restart = locked(&self.restart)?;
            match restart.as_ref() {
                Some((existing, current)) if existing != path || current != config => {
                    return Err(api::invalid(
                        "replica ownership",
                        "client has different reopening configuration",
                    ));
                }
                Some(_) => (),
                None => *restart = Some((path.to_path_buf(), config.clone())),
            }
        }
        Ok(())
    }
    fn resolve(&self, until: Instant) -> Result<Database, Refusal> {
        let mut promoted = locked_until(&self.promoted, until)?;
        if let Some(database) = promoted.as_ref() {
            return Ok(database.clone());
        }
        // Selection sends only authentication and identity probes. No application request or
        // FnOnce is submitted until selection finishes, so this cannot replay an unknown write.
        let restart = locked(&self.restart)?.clone();
        let database = match restart {
            Some((log, config)) => Database::open_replicated(&self.path, &log, config, until)?,
            None => Database::open(&self.path, until)?,
        };
        if !matches!(&database.shared.writer, Writer::Client(_)) {
            *promoted = Some(database.clone());
        }
        Ok(database)
    }
    pub(super) fn replica(&self) -> Option<crate::replication::ReplicaHandle> {
        let promoted = self.promoted.try_lock().ok()?;
        match &promoted.as_ref()?.shared.writer {
            Writer::Replicated { client, .. } => Some(client.handle()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Identity {
    pub restoring: bool,
    pub replica: Option<(String, api::replication::ReplicaConfig)>,
}

pub(super) fn publish(database: &Database, path: &Path) -> Result<(), Refusal> {
    match database.shared.path.set(path.to_path_buf()) {
        Ok(()) => (),
        Err(path) if database.shared.path.get() == Some(&path) => (),
        Err(_) => {
            return Err(api::invalid(
                "database owner",
                "canonical endpoint path changed",
            ));
        }
    }
    if database.shared.restoring.load(Ordering::Acquire) {
        return Ok(());
    }
    if matches!(database.shared.writer, Writer::Client(_)) {
        return Err(api::invalid(
            "database owner",
            "a client cannot publish another owner",
        ));
    }
    let mut service = locked(&database.shared.transport)?;
    if service.is_none() {
        let weak = Arc::downgrade(&database.shared);
        *service = Some(crate::transport::Owner::start(path, move || {
            weak.upgrade().map(|shared| Database { shared })
        })?);
    }
    Ok(())
}

pub(super) fn attach(
    path: &Path,
    replica: Option<(&Path, &api::replication::ReplicaConfig)>,
    until: Instant,
) -> Result<Database, Refusal> {
    let client = crate::transport::Client::connect(path, until)?;
    let identity = validate(&client, replica, until)?;
    let restart = match replica {
        Some((path, config)) => Some((path.to_path_buf(), config.clone())),
        None => identity
            .replica
            .map(|(path, config)| (PathBuf::from(path), config)),
    };
    Ok(Database {
        shared: Arc::new(Shared {
            transport: Mutex::new(None),
            path: OnceLock::new(),
            writer: Writer::Client(Backend {
                client,
                path: path.to_path_buf(),
                restart: Mutex::new(restart),
                promoted: Mutex::new(None),
            }),
            changed: Arc::new(Changed::default()),
            restoring: AtomicBool::new(false),
            admitted: Arc::new(AtomicUsize::new(0)),
            _owner: None,
        }),
    })
}

pub(super) fn validate(
    client: &crate::transport::Client,
    replica: Option<(&Path, &api::replication::ReplicaConfig)>,
    until: Instant,
) -> Result<Identity, Refusal> {
    let identity = client
        .identity(until)
        .map_err(|error| match Refusal::from(error) {
            Refusal::UnknownOutcome => unavailable("owner identity probe interrupted"),
            other => other,
        })?;
    if identity.restoring {
        return Err(api::invalid("database owner", "restore is incomplete"));
    }
    if let Some((path, config)) = replica {
        let matches = identity
            .replica
            .as_ref()
            .is_some_and(|(actual, current)| Path::new(actual) == path && current == config);
        if !matches {
            return Err(api::invalid(
                "replica ownership",
                "the active owner has different replication configuration",
            ));
        }
    }
    Ok(identity)
}

impl Database {
    /// Reads the current owner's replication readiness without SQL or ownership promotion.
    /// None identifies a standalone owner. An unreachable configured owner returns a typed
    /// refusal, so a client cannot mistake an unavailable or bootstrapping replica for readiness.
    pub fn replica_status(
        &self,
        until: Instant,
    ) -> Result<Option<api::replication::ReplicaStatus>, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let status = match &self.shared.writer {
            Writer::Local(_) => None,
            Writer::Replicated { client, .. } => Some(client.handle().status()?),
            Writer::Client(backend) => {
                let configured = locked_until(&backend.restart, until)?.is_some();
                let status = backend.client.replica_status(until).map_err(|error| {
                    match Refusal::from(error) {
                        Refusal::UnknownOutcome => unavailable("owner status probe interrupted"),
                        other => other,
                    }
                })?;
                if configured && status.is_none() {
                    return Err(unavailable(
                        "configured owner replication status unavailable",
                    ));
                }
                status
            }
        };
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        Ok(status)
    }

    /// Attaches to an already-running configured owner without opening or inspecting its engine.
    /// None means no owner holds the lease at this instant; `open_replicated` rechecks ownership
    /// after the host prepares an unowned database. A client retains this full reopening config.
    pub fn attach_replicated(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        until: Instant,
    ) -> Result<Option<Self>, Refusal> {
        Self::attach_owner(path, log_path, config, None, until)
    }

    /// [`Database::attach_replicated`] for a host that owns with `guards`
    /// ([`Database::open_replicated_guarded`]): it makes no directory and never attaches as a
    /// client. It answers this process's live owner opened at this file with guards of the same
    /// key and this configuration, or None when no owner holds the lease; an unguarded owner,
    /// other guards, another file at the path, or another process's owner is refused.
    pub fn attach_replicated_guarded(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        guards: Guards,
        until: Instant,
    ) -> Result<Option<Self>, Refusal> {
        Self::attach_owner(path, log_path, config, Some(guards), until)
    }

    fn attach_owner(
        path: &Path,
        log_path: &Path,
        config: api::replication::ReplicaConfig,
        guards: Option<Guards>,
        until: Instant,
    ) -> Result<Option<Self>, Refusal> {
        if Instant::now() >= until {
            return Err(Refusal::Timeout);
        }
        let (path, log) = match guards {
            Some(_) => (existing_path(path)?, existing_path(log_path)?),
            None => (resolved_path(path)?, resolved_path(log_path)?),
        };
        log.to_str()
            .ok_or_else(|| api::invalid("replica path", "not UTF-8"))?;
        let expected = Some((log.as_path(), &config));
        let mut owners = locked_until(OWNERS.get_or_init(Mutex::default), until)?;
        owners.retain(|_, owner| owner.shared.strong_count() > 0 || owner.lease.strong_count() > 0);
        let identity = file_identity(&path)?;
        if let Some(owner) = owners.get(&path).or_else(|| {
            identity.and_then(|id| owners.values().find(|owner| owner.identity == Some(id)))
        }) {
            if let Some(shared) = owner.shared.upgrade() {
                if let Some(guards) = &guards {
                    admit_guarded(owner, &shared, identity, guards)?;
                }
                if shared.restoring.load(Ordering::Acquire) {
                    return Err(api::invalid("replica ownership", "restore is incomplete"));
                }
                match &shared.writer {
                    Writer::Client(backend) => match validate(&backend.client, expected, until) {
                        Ok(_) => {
                            backend.configure(expected)?;
                            return Ok(Some(Self { shared }));
                        }
                        Err(
                            Refusal::Unavailable(_) | Refusal::Timeout | Refusal::UnknownOutcome,
                        ) => (),
                        Err(error) => return Err(error),
                    },
                    Writer::Replicated {
                        log_path,
                        config: current,
                        ..
                    } if log_path == &log && current == &config => {
                        return Ok(Some(Self { shared }));
                    }
                    _ => {
                        return Err(api::invalid(
                            "replica ownership",
                            "a different live owner must drain",
                        ));
                    }
                }
            }
            if owner.lease.strong_count() > 0 {
                return Err(unavailable("previous database owner is draining"));
            }
        }
        // Admission before the probe: a probe makes the endpoint directory and takes the lease,
        // which a full table would refuse to anyone who went on to open.
        api::bound("open database owners", owners.len().saturating_add(1), 4096)?;
        // No owner holds a lease under a directory that doesn't exist; attach makes no directory.
        if !parent(&path).try_exists().map_err(unavailable)? {
            return Ok(None);
        }
        let lease = match guards {
            // The path is canonical and its parent exists: the probe makes no directory above.
            Some(_) => crate::transport::try_lease(&path)?,
            None => try_exclusive_file(&path)?,
        };
        if let Some(lease) = lease {
            drop(lease);
            return Ok(None);
        }
        if guards.is_some() {
            return Err(remote_owner());
        }
        let database = attach(&path, expected, until)?;
        owners.insert(
            path,
            Owner {
                shared: Arc::downgrade(&database.shared),
                lease: Weak::new(),
                identity,
                key: None,
            },
        );
        Ok(Some(database))
    }

    pub(super) fn resolve(&self, until: Instant) -> Result<Database, Refusal> {
        match &self.shared.writer {
            Writer::Client(backend) => backend.resolve(until),
            _ => Ok(self.clone()),
        }
    }
    /// Selects the current host owner before carrying replication messages. A standby returns
    /// None; after its predecessor releases ownership this handle starts the same configured
    /// product actor and returns its carrier handle. Submitted application calls are never retried.
    pub fn ensure_owner(
        &self,
        until: Instant,
    ) -> Result<Option<crate::replication::ReplicaHandle>, Refusal> {
        let database = self.resolve(until)?;
        Ok(match &database.shared.writer {
            Writer::Replicated { client, .. } => Some(client.handle()),
            Writer::Local(_) | Writer::Client(_) => None,
        })
    }
    pub(crate) fn transport_identity(&self) -> Result<Identity, Refusal> {
        let replica = match &self.shared.writer {
            Writer::Replicated {
                log_path, config, ..
            } => Some((
                log_path
                    .to_str()
                    .ok_or_else(|| api::invalid("replica path", "not UTF-8"))?
                    .to_owned(),
                config.clone(),
            )),
            Writer::Local(_) => None,
            Writer::Client(_) => {
                return Err(api::invalid(
                    "database owner",
                    "client handle cannot serve another client",
                ));
            }
        };
        Ok(Identity {
            restoring: self.shared.restoring.load(Ordering::Acquire),
            replica,
        })
    }
    pub(super) fn remote_client(&self) -> Option<&crate::transport::Client> {
        match &self.shared.writer {
            Writer::Client(backend) => Some(&backend.client),
            _ => None,
        }
    }
}
impl Namespace {
    pub(super) fn remote(&self) -> Result<Option<crate::transport::Namespace>, Refusal> {
        self.database
            .remote_client()
            .map(|client| client.namespace(&self.name))
            .transpose()
    }
}

pub(super) fn submit(
    client: &crate::transport::Client,
    namespace: &str,
    operation: Operation,
    until: Instant,
) -> Result<Answer, Refusal> {
    match operation {
        Operation::Mutation(Mutation::Sql(statements)) => {
            return client
                .execute(statements, until)
                .map(Answer::Sql)
                .map_err(Refusal::from);
        }
        Operation::Mutation(Mutation::Migrate(migration)) => {
            return client
                .migrate(migration, until)
                .map(Answer::Migrated)
                .map_err(Refusal::from);
        }
        Operation::Call(_) => {
            return Err(api::invalid(
                "SQL session",
                "client calls must use the public SQL session before capturing the closure",
            ));
        }
        _ => (),
    }
    let records = client.namespace(namespace)?;
    match operation {
        Operation::Mutation(Mutation::Records { tx, .. }) => {
            records.commit(tx, until).map(Answer::Rev)
        }
        Operation::Mutation(Mutation::Compact { before, .. }) => {
            records.compact(before, until)?;
            Ok(Answer::Done)
        }
        Operation::Mutation(Mutation::CompactStep { before, .. }) => {
            records.compact_step(before, until).map(Answer::Compacted)
        }
        Operation::Get(family, key, at) => records.get(&family, &key, at, until).map(Answer::Row),
        Operation::Scan(family, scan) => records.scan(&family, scan, until).map(Answer::Page),
        Operation::Changes(family, after, limit) => records
            .changes(&family, after, limit, until)
            .map(Answer::Changes),
        Operation::Head => records.head(until).map(Answer::Rev),
        Operation::Import(page) => {
            records.import_page(page, until)?;
            Ok(Answer::Done)
        }
        Operation::ImportProgress => records.import_state(until).map(Answer::ImportProgress),
        Operation::Call(_) | Operation::Mutation(Mutation::Sql(_) | Mutation::Migrate(_)) => {
            Err(api::invalid("client operation", "unexpected dispatch"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn until() -> Instant {
        Instant::now() + crate::transport::HANG
    }

    fn config() -> api::replication::ReplicaConfig {
        api::replication::ReplicaConfig {
            node_id: 1,
            voters: vec![1],
            learners: vec![],
            bootstrap: None,
        }
    }

    /// The entries of `dir`, to compare before and after an open.
    fn entries(dir: &Path) -> Vec<std::ffi::OsString> {
        let mut names: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn refused_replicated_opens_leave_no_directory_they_made() {
        // CP-T1.5 (U05), local-owner-transport.md "Owner guards": an unguarded open_replicated
        // or attach_replicated refused by the owner table or by its configuration makes no
        // directory first, and an attach that finds no owner makes none either.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db");
        let owner =
            Database::open_replicated(&path, &dir.path().join("log"), config(), until()).unwrap();
        let before = entries(dir.path());
        let elsewhere = dir.path().join("new").join("log");
        let refused = Database::open_replicated(&path, &elsewhere, config(), until());
        assert!(
            matches!(refused, Err(Refusal::Invalid { .. })),
            "{:?}",
            refused.err()
        );
        let refused = Database::attach_replicated(&path, &elsewhere, config(), until());
        assert!(
            matches!(refused, Err(Refusal::Invalid { .. })),
            "{:?}",
            refused.err()
        );
        let invalid = api::replication::ReplicaConfig {
            node_id: 0,
            ..config()
        };
        let (missing_db, missing_log) = (
            dir.path().join("a").join("db"),
            dir.path().join("b").join("log"),
        );
        let refused = Database::open_replicated(&missing_db, &missing_log, invalid, until());
        assert!(
            matches!(refused, Err(Refusal::Invalid { .. })),
            "{:?}",
            refused.err()
        );
        let attached = Database::attach_replicated(&missing_db, &missing_log, config(), until());
        assert!(
            matches!(attached, Ok(None)),
            "{:?}",
            attached.map(|d| d.is_some())
        );
        assert_eq!(
            entries(dir.path()),
            before,
            "a refused open made a directory"
        );
        drop(owner);
    }

    #[test]
    fn opens_refused_at_the_owner_bound_make_no_directory() {
        // CP-T1.5 (U05): with the owner table full, open, open_replicated and attach_replicated
        // under directories that don't exist are refused at the bound and make none of them.
        if !in_child("database::remote::tests::opens_refused_at_the_owner_bound_make_no_directory")
        {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let held = Arc::new(File::create(dir.path().join("held")).unwrap());
        {
            let mut owners = OWNERS.get_or_init(Mutex::default).lock().unwrap();
            for n in 0..4096 {
                owners.insert(
                    dir.path().join(format!("full-{n}")),
                    Owner {
                        shared: Weak::new(),
                        lease: Arc::downgrade(&held),
                        identity: None,
                        key: None,
                    },
                );
            }
        }
        let before = entries(dir.path());
        let (db, log) = (
            dir.path().join("a").join("db"),
            dir.path().join("b").join("log"),
        );
        let full = |refused: Result<(), Refusal>| {
            assert!(
                matches!(refused, Err(Refusal::TooLarge { limit: 4096, .. })),
                "{refused:?}"
            );
        };
        full(Database::open(&db, until()).map(drop));
        full(Database::open_replicated(&db, &log, config(), until()).map(drop));
        full(Database::attach_replicated(&db, &log, config(), until()).map(drop));
        assert_eq!(
            entries(dir.path()),
            before,
            "a refused open made a directory"
        );
    }

    #[test]
    fn attach_refuses_at_the_owner_bound_before_it_touches_the_lease() {
        // CP-T1.5 (U05): attach with the owner table full, on a database no process owns, is
        // refused at the bound and leaves no endpoint directory or lease behind.
        if !in_child(
            "database::remote::tests::attach_refuses_at_the_owner_bound_before_it_touches_the_lease",
        ) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let held = Arc::new(File::create(dir.path().join("held")).unwrap());
        {
            let mut owners = OWNERS.get_or_init(Mutex::default).lock().unwrap();
            for n in 0..4096 {
                owners.insert(
                    dir.path().join(format!("full-{n}")),
                    Owner {
                        shared: Weak::new(),
                        lease: Arc::downgrade(&held),
                        identity: None,
                        key: None,
                    },
                );
            }
        }
        let path = dir.path().join("db");
        let config = api::replication::ReplicaConfig {
            node_id: 1,
            voters: vec![1],
            learners: vec![],
            bootstrap: None,
        };
        let attached = Database::attach_replicated(&path, &dir.path().join("log"), config, until());
        assert!(
            matches!(attached, Err(Refusal::TooLarge { limit: 4096, .. })),
            "attach at the bound: {:?}",
            attached.map(|d| d.is_some())
        );
        let mut sidecar = path.into_os_string();
        sidecar.push(".client");
        assert!(
            !Path::new(&sidecar).exists(),
            "the refused attach made the endpoint directory"
        );
    }
}
