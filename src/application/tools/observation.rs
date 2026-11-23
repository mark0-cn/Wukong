//! Fair bounded resource observation; controller and environment leases run separately.
use super::*;
use std::collections::{BTreeSet, VecDeque};
use tokio::{task::JoinSet, time::Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
enum Resource {
    Job(String),
    Session(SessionKind, String),
}
#[derive(Clone)]
struct Candidate {
    key: String,
    environment: String,
    resource: Resource,
}
struct Entry {
    environment: String,
    due: Instant,
    failures: u32,
    last: u64,
    running: bool,
}
struct FairObserver {
    entries: BTreeMap<String, Entry>,
    environments: VecDeque<String>,
    sequence: u64,
    global: usize,
    per_environment: usize,
}
impl FairObserver {
    fn new(global: usize, per_environment: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            environments: VecDeque::new(),
            sequence: 0,
            global,
            per_environment,
        }
    }
    fn select(&mut self, candidates: &[Candidate], now: Instant) -> Vec<Candidate> {
        let live: BTreeSet<_> = candidates.iter().map(|c| c.key.clone()).collect();
        self.entries
            .retain(|id, entry| entry.running || live.contains(id));
        let environments: BTreeSet<_> = candidates.iter().map(|c| c.environment.clone()).collect();
        self.environments.retain(|env| environments.contains(env));
        for env in environments {
            if !self.environments.contains(&env) {
                self.environments.push_back(env);
            }
        }
        for candidate in candidates {
            self.entries.entry(candidate.key.clone()).or_insert(Entry {
                due: now,
                failures: 0,
                last: 0,
                running: false,
                environment: candidate.environment.clone(),
            });
        }
        let mut active = self.entries.values().filter(|entry| entry.running).count();
        let mut selected = vec![];
        let mut misses = 0;
        while active < self.global && misses < self.environments.len() {
            let Some(environment) = self.environments.pop_front() else {
                break;
            };
            self.environments.push_back(environment.clone());
            let count = self
                .entries
                .values()
                .filter(|entry| entry.environment == environment && entry.running)
                .count();
            let candidate = if count < self.per_environment {
                candidates
                    .iter()
                    .filter(|c| {
                        c.environment == environment
                            && !self.entries[&c.key].running
                            && self.entries[&c.key].due <= now
                    })
                    .min_by_key(|c| self.entries[&c.key].last)
            } else {
                None
            };
            if let Some(candidate) = candidate {
                self.sequence += 1;
                let entry = self
                    .entries
                    .get_mut(&candidate.key)
                    .expect("the selected candidate was read from entries above");
                entry.running = true;
                entry.last = self.sequence;
                active += 1;
                misses = 0;
                selected.push(candidate.clone());
            } else {
                misses += 1;
            }
        }
        selected
    }
    fn finished(&mut self, key: &str, success: bool, now: Instant) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.running = false;
            entry.failures = if success {
                0
            } else {
                entry.failures.saturating_add(1)
            };
            entry.due = now
                + Duration::from_secs(if success {
                    1
                } else {
                    (1_u64 << entry.failures.saturating_sub(1).min(5)).min(30)
                });
        }
    }
}

impl ToolService {
    fn observation_candidates(&self, run: &str, sync: bool) -> Result<Vec<Candidate>> {
        let agents: BTreeMap<_, _> = self
            .store
            .records::<AgentMetadata>(run)?
            .into_iter()
            .map(|a| (a.id.clone(), a))
            .collect();
        let mut candidates = vec![];
        for job in self.store.records::<JobRecord>(run)? {
            let Some(owner) = agents.get(&job.agent_id) else {
                continue;
            };
            let needed = if sync {
                job.status.settled()
                    && job.notification_message_id.is_none()
                    && job.evidence_sync.state != SyncState::Failed
                    && job.evidence_sync.retry_at.is_none_or(|at| at <= now())
            } else {
                !job.status.settled()
                    || job.status_evidence_id.is_none()
                    || (job.execution_notification_message_id.is_none()
                        && job.notification_message_id.is_none())
            };
            if needed {
                candidates.push(Candidate {
                    key: format!("job:{}", job.id),
                    environment: owner.environment_id.clone().unwrap_or_default(),
                    resource: Resource::Job(job.id),
                });
            }
        }
        if !sync {
            for kind in [SessionKind::Debugger, SessionKind::Interactive] {
                for session in self.store.sessions(kind, run)? {
                    let Some(owner) = agents.get(&session.agent_id) else {
                        continue;
                    };
                    if !matches!(session.state, SessionState::Closed | SessionState::Lost)
                        && (matches!(
                            session.state,
                            SessionState::Closing | SessionState::RecoveryRequired
                        ) || matches!(owner.status, Status::Stopped | Status::Failed))
                    {
                        candidates.push(Candidate {
                            key: format!("session:{}", session.id),
                            environment: owner.environment_id.clone().unwrap_or_default(),
                            resource: Resource::Session(kind, session.id),
                        });
                    }
                }
            }
        }
        Ok(candidates)
    }
    async fn observe_candidate(&self, run: &str, resource: Resource, sync: bool) -> Result<()> {
        // Boxed so each monitor task holds a small future instead of the whole
        // observe/reconcile state machine (about 24 KiB per candidate task).
        let operation = Box::pin(async {
            match resource {
                Resource::Job(id) => self.observe_job(run, &id, sync).await,
                Resource::Session(kind, id) => self.reconcile_session(run, kind, &id).await,
            }
        });
        if sync {
            operation.await
        } else {
            tokio::time::timeout(Duration::from_secs(10), operation)
                .await
                .context("resource observation timed out")?
        }
    }

    pub(crate) async fn monitor_resources(&self, run: &str, cancel: CancellationToken) {
        tokio::join!(
            self.monitor_lane(run, false, cancel.clone()),
            self.monitor_lane(run, true, cancel)
        );
    }
    async fn monitor_lane(&self, run: &str, sync: bool, cancel: CancellationToken) {
        let mut fair = if sync {
            FairObserver::new(2, 1)
        } else {
            FairObserver::new(8, 2)
        };
        let mut tasks: JoinSet<(String, Result<()>)> = JoinSet::new();
        let mut timer = tokio::time::interval(Duration::from_millis(100));
        let mut candidates = vec![];
        let mut next_scan = Instant::now();
        loop {
            tokio::select! {
                biased;
                _=cancel.cancelled()=>break,
                result=tasks.join_next(), if !tasks.is_empty()=> {
                    match result {
                        Some(Ok((key,result)))=> {
                            fair.finished(&key,result.is_ok(),Instant::now());
                            if let Err(error)=result {tracing::warn!(resource=%key,%error,"resource observation deferred");}
                        }
                        Some(Err(error))=> { tracing::error!(%error,"resource observer panicked"); break; }
                        None=>{}
                    }
                }
                _=timer.tick()=> {
                    if Instant::now()>=next_scan {
                        match self.observation_candidates(run,sync) {
                            Ok(current)=>candidates=current,
                            Err(error)=>tracing::warn!(%error,"resource inventory unavailable"),
                        }
                        next_scan=Instant::now()+Duration::from_secs(1);
                    }
                    for candidate in fair.select(&candidates,Instant::now()) {
                        let tools=self.clone();
                        let run=run.to_owned();
                        tasks.spawn(async move {
                            let result=tools.observe_candidate(&run,candidate.resource,sync).await;
                            (candidate.key,result)
                        });
                    }
                }
            }
        }
        // Dropping JoinSet cancels in-flight IO; the fenced Store rejects late
        // results and durable cleanup/synchronization checkpoints survive.
    }

    pub(super) async fn observe_once(&self, run: &str) -> Result<()> {
        let mut candidates = self.observation_candidates(run, false)?;
        // The shutdown pass only reconciles cleanup intent. Paused/live jobs
        // remain owned and must not delay returning control to the user.
        candidates.retain(|candidate| match &candidate.resource {
            Resource::Session(..) => true,
            Resource::Job(id) => self
                .store
                .load::<JobRecord>(id)
                .and_then(|job| self.store.load::<AgentMetadata>(&job.agent_id))
                .is_ok_and(|owner| matches!(owner.status, Status::Stopped | Status::Failed)),
        });
        let mut fair = FairObserver::new(8, 2);
        let mut tasks: JoinSet<(String, Result<()>)> = JoinSet::new();
        loop {
            for candidate in fair.select(&candidates, Instant::now()) {
                let tools = self.clone();
                let run = run.to_owned();
                tasks.spawn(async move {
                    (
                        candidate.key,
                        tools
                            .observe_candidate(&run, candidate.resource, false)
                            .await,
                    )
                });
            }
            let Some(result) = tasks.join_next().await else {
                break;
            };
            let (key, result) = result.context("resource observer panicked")?;
            fair.finished(&key, result.is_ok(), Instant::now());
            candidates.retain(|candidate| candidate.key != key);
            if let Err(error) = result {
                tracing::warn!(resource=%key,%error,"resource cleanup remains pending");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_and_stalled_environments_do_not_block_completion_or_cancellation() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let bridge = dir.path().join("observer.py");
        fs::write(
            &bridge,
            r#"import sys,json,pathlib,time,platform
root=pathlib.Path(sys.argv[sys.argv.index('--state')+1]);root.mkdir(exist_ok=True)
identity={'protocol':4,'installation_id':root.name,'boot_id':'boot','package_sha256':'0'*64}
for line in sys.stdin:
 r=json.loads(line);method=r['method']
 if method=='ping': value={'identity':identity}
 elif method=='doctor': value={'os':platform.system().lower()}
 elif method=='job.status':
  (root/'observed').write_text('yes')
  if root.name=='stalled': time.sleep(60)
  if root.name=='failed':
   print(json.dumps({'jsonrpc':'2.0','id':r['id'],'error':{'code':-32000,'message':'fixture failure','data':{'kind':'internal'}}}),flush=True);continue
  value={'job_id':'job','state':'completed'}
 elif method=='job.artifacts': value=[]
 else: value={}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':value}),flush=True)
"#,
        )?;
        let mut config = AppConfig::default();
        for environment in ["stalled", "failed", "healthy"] {
            config.environments.insert(
                environment.into(),
                crate::adapters::environments::EnvironmentConfig {
                    name: environment.into(),
                    platform: crate::adapters::environments::Platform::current(),
                    installation_id: None,
                    runtime: crate::adapters::worker::RuntimeConfig {
                        transport: crate::adapters::worker::TransportConfig::TrustedLocal(
                            crate::adapters::worker::TrustedLocalConfig {
                                state_dir: dir.path().join(environment),
                                bridge_path: bridge.clone(),
                                ..Default::default()
                            },
                        ),
                        ..Default::default()
                    },
                },
            );
        }
        config.default_environment = Some("healthy".into());
        let store = Store::open(dir.path().join("state"))?;
        let coordinator = Coordinator::new(store.clone());
        let target = dir.path().join("inert");
        fs::write(&target, b"fixture")?;
        let run = coordinator.create_run(
            &target,
            "observer fixture",
            Budget::default(),
            serde_json::to_value(&config)?,
        )?;
        let tools = ToolService::new(config, store.clone()).await?;
        for environment in ["stalled", "failed", "healthy"] {
            let mut owner = coordinator
                .create_child(
                    &run.root_id,
                    serde_json::from_value(json!({
                        "name":environment,"role":"analyst","task":environment,"scope":environment
                    }))?,
                )?
                .agent
                .metadata;
            owner.environment_id = Some(environment.into());
            store.save_record(&owner)?;
            let context = tools.bound_to(&owner).await?;
            let job: JobRecord = serde_json::from_value(
                json!({"id":environment,"worker_resource_id":"job",
                "run_id":run.id,"agent_id":owner.id,"worker_id":context.worker_identity()?,"owner_generation":owner.generation,
                "tool_name":"job_start","call_id":"start","worker_request_id":"start","parameters":{},
                "target_sha256":run.target_sha256,"status":{"job_id":"job","state":"running"},
                "created_at":now(),"updated_at":now()}),
            )?;
            store.save_record(&job)?;
        }
        let cancel = CancellationToken::new();
        let (monitor, run_id, stop) = (tools.clone(), run.id.clone(), cancel.clone());
        let monitor = tokio::spawn(async move { monitor.monitor_resources(&run_id, stop).await });
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if store
                    .load::<JobRecord>("healthy")?
                    .notification_message_id
                    .is_some()
                    && dir.path().join("failed/observed").exists()
                    && dir.path().join("stalled/observed").exists()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        assert!(!store.load::<JobRecord>("stalled")?.evidence_ready());
        assert!(!store.load::<JobRecord>("failed")?.evidence_ready());
        cancel.cancel();
        tokio::time::timeout(Duration::from_secs(1), monitor).await??;
        tools.close().await;
        Ok(())
    }
    fn candidates() -> Vec<Candidate> {
        (0..5)
            .flat_map(|env| {
                (0..5).map(move |item| Candidate {
                    key: format!("{env}:{item}"),
                    environment: env.to_string(),
                    resource: Resource::Job(format!("{env}:{item}")),
                })
            })
            .collect()
    }
    #[test]
    fn bounded_fair_deduplicated_and_backoff_resets() {
        let input = candidates();
        let now = Instant::now();
        let mut fair = FairObserver::new(8, 2);
        let first = fair.select(&input, now);
        assert_eq!(first.len(), 8);
        assert_eq!(
            first
                .iter()
                .map(|c| &c.environment)
                .collect::<BTreeSet<_>>()
                .len(),
            5
        );
        assert!(fair.select(&input, now).is_empty());
        for candidate in &first {
            fair.finished(&candidate.key, candidate.environment != "0", now);
        }
        let second = fair.select(&input, now);
        assert_eq!(second.len(), 8);
        assert!(
            second
                .iter()
                .all(|c| !first.iter().any(|old| old.key == c.key))
        );
        let key = &first[0].key;
        for power in 1..10 {
            fair.finished(key, false, now);
            assert_eq!(
                fair.entries[key].due - now,
                Duration::from_secs((1_u64 << power.min(5)).min(30))
            );
        }
        fair.finished(key, true, now);
        assert_eq!(fair.entries[key].failures, 0);
        assert_eq!(fair.entries[key].due - now, Duration::from_secs(1));
    }
}
