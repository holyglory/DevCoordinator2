//! Real database evidence. Every test runs against the owned socket-only
//! fixture runtime, never the installed Coordinator or another agent's DB.

use devcoordinator2_persistence::{
    Error, Lane, Persistence, PoolSizes, Result, backup, fixture::FixtureCluster, params,
};
use std::time::{Duration, Instant};

fn origin_upgrade_fixture(cluster: &FixtureCluster, name: &str, prefix: i64) -> Persistence {
    use devcoordinator2_persistence::schema::MIGRATIONS;
    use sha2::{Digest, Sha256};
    let database = Persistence::connect(&cluster.create_database(name).unwrap()).unwrap();
    database.maintenance_transaction::<_,Error,_>(|tx|{
        tx.execute_batch("CREATE TABLE dc2_schema_migrations(version BIGINT PRIMARY KEY,name TEXT NOT NULL,sha256 TEXT NOT NULL,applied_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp())")?;
        for (version,name,sql) in MIGRATIONS.iter().filter(|(version,_,_)|*version<=prefix) {
            tx.execute_batch(sql)?;
            let checksum=Sha256::digest(sql.as_bytes()).iter().map(|byte|format!("{byte:02x}")).collect::<String>();
            tx.execute("INSERT INTO dc2_schema_migrations(version,name,sha256) VALUES($1,$2,$3)",params![version,name,checksum])?;
        }
        tx.execute_batch("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('origin-a','/fixture/source-a','A','registered-a',1000,'seen-a'),('origin-b','/fixture/source-b','B','registered-b',1000,'seen-b');
            INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES('primary-a','origin-a','/fixture/source-a','registered-a','seen-a'),('primary-b','origin-b','/fixture/source-b','registered-b','seen-b');
            INSERT INTO deployments(deployment_id,repository_id,worktree_id,name,source,spec_fingerprint,spec_json,state,created_at,created_by_uid,client,updated_at) VALUES('deployment-a','origin-a','primary-a','A','checkout','fixture','{}','stopped','deployment-a-created',1000,'fixture','deployment-a-updated'),('deployment-b','origin-b','primary-b','B','checkout','fixture','{}','stopped','deployment-b-created',1000,'fixture','deployment-b-updated')")?;
        Ok(())
    }).unwrap();
    database
}

fn legacy_origin(
    connection: &devcoordinator2_persistence::Connection,
    deployment: &str,
    number: i64,
    worktree: &str,
    path: &str,
    created: &str,
) -> Result<()> {
    connection.execute("INSERT INTO generations(deployment_id,number,commit_hash,dirty,path,fingerprint,created_at,state) VALUES($1,$2,'preserved-commit',0,$3,'preserved-fingerprint',$4,'historical')",params![deployment,number,format!("/fixture/generation-{deployment}-{number}"),created])?;
    connection.execute("INSERT INTO generation_source_origins(deployment_id,number,worktree_id,path) VALUES($1,$2,$3,$4)",params![deployment,number,worktree,path])?;
    Ok(())
}

fn origin_history(database: &Persistence) -> serde_json::Value {
    database.read::<_,Error,_>(|c|c.query_row("SELECT jsonb_build_object('origins',(SELECT jsonb_agg(to_jsonb(o) ORDER BY deployment_id,number) FROM generation_source_origins o),'generations',(SELECT jsonb_agg(to_jsonb(g) ORDER BY deployment_id,number) FROM generations g),'deployments',(SELECT jsonb_agg(to_jsonb(d) ORDER BY deployment_id) FROM deployments d))",[],|row|row.get(0))).unwrap()
}

fn worktree_rows(database: &Persistence) -> serde_json::Value {
    database
        .read::<_, Error, _>(|c| {
            c.query_row(
                "SELECT jsonb_agg(to_jsonb(w) ORDER BY worktree_id) FROM worktrees w",
                [],
                |row| row.get(0),
            )
        })
        .unwrap()
}

fn migration_refusal(error: &Error) -> Option<&str> {
    match error {
        Error::Postgres(error) => error
            .as_db_error()
            .filter(|error| error.code() == &postgres::error::SqlState::CHECK_VIOLATION)
            .map(|error| error.message()),
        _ => None,
    }
}

#[test]
fn legacy_candidate_origins_backfill_stable_ids_without_rewriting_history() {
    let cluster = FixtureCluster::start().unwrap();
    // This is the actual live schema35 migration prefix, before any Explorer
    // tables or native registration bindings existed.
    let database = origin_upgrade_fixture(&cluster, "origin_backfill", 16);
    database.write::<_,Error,_>(|c|{
        legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/candidate-a","2026-11-16T12:00:00Z")?;
        legacy_origin(c,"deployment-a",2,"w1111111111111111","/fixture/candidate-a","2026-11-17T12:00:00Z")?;
        c.execute("INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES('registered-source','origin-a','/fixture/registered','keep-registration','keep-last-seen')",[])?;
        legacy_origin(c,"deployment-a",3,"registered-source","/fixture/registered","2026-11-17T13:00:00Z")?;
        Ok(())
    }).unwrap();
    let history = origin_history(&database);
    let existing = worktree_rows(&database);
    database.migrate().unwrap();
    assert_eq!(origin_history(&database), history);
    database.read::<_,Error,_>(|c|{
        let row=c.query_row("SELECT repository_id,worktree_path,registered_at,last_seen_at FROM worktrees WHERE worktree_id='w1111111111111111'",[],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?;
        assert_eq!(row,("origin-a".into(),"/fixture/candidate-a".into(),"2026-11-16T12:00:00Z".into(),"2026-11-17T12:00:00Z".into()));
        let unchanged:serde_json::Value=c.query_row("SELECT jsonb_agg(to_jsonb(w) ORDER BY worktree_id) FROM worktrees w WHERE worktree_id<>'w1111111111111111'",[],|row|row.get(0))?;
        assert_eq!(unchanged,existing,"existing registration IDs, paths, timestamps and row identities are preserved");
        assert_eq!(c.query_row("SELECT count(*) FROM worktree_native_identities",[],|row|row.get::<_,i64>(0))?,0,"SQL backfill never grants native identity proof");
        assert_eq!(c.query_row("SELECT count(*) FROM repository_native_identities",[],|row|row.get::<_,i64>(0))?,0);
        assert_eq!(c.query_row("SELECT value FROM meta WHERE key='schema_version'",[],|row|row.get::<_,String>(0))?,devcoordinator2_persistence::schema::DATABASE_SCHEMA_VERSION.to_string());
        Ok(())
    }).unwrap();
    let after = worktree_rows(&database);
    database.migrate().unwrap();
    assert_eq!(
        worktree_rows(&database),
        after,
        "repeated migration cannot allocate a fresh ID or registration row"
    );
    assert_eq!(origin_history(&database), history);
}

#[test]
fn legacy_candidate_origins_refuse_collisions_and_rollback_the_upgrade() {
    let cluster = FixtureCluster::start().unwrap();
    let cases = [
        (
            "origin_owner_collision",
            "owner",
            "legacy candidate origin identity has conflicting repository ownership",
        ),
        (
            "origin_path_collision",
            "path",
            "legacy candidate origin path has conflicting source identities",
        ),
        (
            "origin_ambiguous_paths",
            "paths",
            "unregistered legacy candidate origin has ambiguous source paths",
        ),
        (
            "origin_registered_owner",
            "registered-owner",
            "candidate origin conflicts with its registered source or committed relocation",
        ),
        (
            "origin_registered_path",
            "registered-path",
            "candidate origin conflicts with its registered source or committed relocation",
        ),
        (
            "origin_claimed_path",
            "claimed-path",
            "candidate origin path is already owned by another registered source",
        ),
        (
            "origin_invalid_id",
            "invalid-id",
            "unregistered candidate origin identity is not a legacy producer identity",
        ),
        (
            "origin_foreign_root",
            "foreign-root",
            "candidate origin path is already owned by another registered source",
        ),
    ];
    let mut failures = Vec::new();
    for (name, case, expected) in cases {
        let database = origin_upgrade_fixture(&cluster, name, 16);
        database.write::<_,Error,_>(|c|{
            match case {
                "owner"=>{
                    legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/candidate-a","t")?;
                    legacy_origin(c,"deployment-b",1,"w1111111111111111","/fixture/candidate-b","t")?;
                },
                "path"=>{
                    legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/candidate-a","t")?;
                    legacy_origin(c,"deployment-a",2,"w2222222222222222","/fixture/candidate-a","t")?;
                },
                "paths"=>{
                    legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/candidate-a","t")?;
                    legacy_origin(c,"deployment-a",2,"w1111111111111111","/fixture/candidate-b","t")?;
                },
                "registered-owner"=>legacy_origin(c,"deployment-b",1,"primary-a","/fixture/source-a","t")?,
                "registered-path"=>legacy_origin(c,"deployment-a",1,"primary-a","/fixture/candidate-a","t")?,
                "claimed-path"=>legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/source-a","t")?,
                "invalid-id"=>legacy_origin(c,"deployment-a",1,"unregistered-source","/fixture/candidate-a","t")?,
                "foreign-root"=>{
                    c.execute("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('origin-c','/fixture/foreign-root','C','t',1000,'t')",[])?;
                    legacy_origin(c,"deployment-a",1,"w1111111111111111","/fixture/foreign-root","t")?;
                },
                _=>unreachable!(),
            }
            Ok(())
        }).unwrap();
        let history = origin_history(&database);
        let registrations = worktree_rows(&database);
        match database.migrate() {
            Err(error) if migration_refusal(&error) == Some(expected) => {}
            Err(_) => failures.push(format!("{case}: unexpected migration refusal")),
            Ok(()) => failures.push(format!("{case}: ambiguous provenance was accepted")),
        }
        if worktree_rows(&database) != registrations || origin_history(&database) != history {
            failures.push(format!(
                "{case}: failed upgrade changed protected authority"
            ));
        }
        let (version,migration):(String,i64)=database.read::<_,Error,_>(|c|c.query_row("SELECT (SELECT value FROM meta WHERE key='schema_version'),(SELECT max(version) FROM dc2_schema_migrations)",[],|row|Ok((row.get(0)?,row.get(1)?)))).unwrap();
        if version != "35" || migration != 16 {
            failures.push(format!(
                "{case}: failed upgrade was not rolled back to the exact schema35 prefix"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

#[test]
fn legacy_candidate_origins_preserve_only_committed_relocation_aliases() {
    let cluster = FixtureCluster::start().unwrap();
    for (name, state) in [
        ("origin_committed_alias", "committed"),
        ("origin_uncommitted_alias", "verified"),
    ] {
        // Relocation history first exists after the old live prefix. Exercise
        // its already deployed rows without changing any applied SQL checksum.
        let database = origin_upgrade_fixture(&cluster, name, 20);
        database.write::<_,Error,_>(|c|{
            c.execute_batch("INSERT INTO projects(project_id,name,slug,created_at,updated_at,updated_by) VALUES('origin-project','Origin','origin','t','t','fixture');
                INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) VALUES('registered-source','origin-a','/fixture/current-source','keep-registration','keep-last-seen')")?;
            for (id,old,new) in [("origin-move-1","/fixture/old-source","/fixture/middle-source"),("origin-move-2","/fixture/middle-source","/fixture/current-source")] {
                c.execute("INSERT INTO repository_relocations(relocation_id,project_id,repository_id,worktree_id,old_path,new_path,moves_repository_root,idempotency_key,request_json,fingerprint,native_json,state,checkout_revision,actor_uid,prepared_at,verified_at,committed_at) VALUES($1,'origin-project','origin-a','registered-source',$2,$3,false,$1,'{}',repeat('a',64),'{}',$4,1,1000,'prepared','verified',$5)",params![id,old,new,if id=="origin-move-1"{state}else{"committed"},(id!="origin-move-1"||state=="committed").then_some("committed")])?;
                c.execute("INSERT INTO registry_path_aliases(old_path,repository_id,worktree_id,new_path,relocation_id,created_at) VALUES($1,'origin-a','registered-source',$2,$3,'t')",params![old,new,id])?;
            }
            legacy_origin(c,"deployment-a",1,"registered-source","/fixture/old-source","first-generation")?;
            legacy_origin(c,"deployment-a",2,"registered-source","/fixture/middle-source","second-generation")?;
            Ok(())
        }).unwrap();
        let registrations = worktree_rows(&database);
        let history = origin_history(&database);
        let aliases = database
            .read::<_, Error, _>(|c| {
                c.query_row(
                    "SELECT jsonb_agg(to_jsonb(a) ORDER BY old_path) FROM registry_path_aliases a",
                    [],
                    |row| row.get::<_, serde_json::Value>(0),
                )
            })
            .unwrap();
        let result = database.migrate();
        if state == "committed" {
            result.unwrap();
            database.migrate().unwrap();
        } else {
            assert_eq!(
                migration_refusal(&result.unwrap_err()),
                Some(
                    "candidate origin conflicts with its registered source or committed relocation"
                )
            );
        }
        assert_eq!(worktree_rows(&database), registrations);
        assert_eq!(origin_history(&database), history);
        assert_eq!(
            database
                .read::<_, Error, _>(|c| c.query_row(
                    "SELECT jsonb_agg(to_jsonb(a) ORDER BY old_path) FROM registry_path_aliases a",
                    [],
                    |row| row.get::<_, serde_json::Value>(0)
                ))
                .unwrap(),
            aliases
        );
    }
}

fn request_index_upgrade_preserves_native_rows(cluster: &FixtureCluster) {
    use devcoordinator2_persistence::schema::MIGRATIONS;
    use sha2::{Digest, Sha256};

    let mut config = cluster.create_database("storage_upgrade").unwrap();
    let mut observer = cluster.client("storage_upgrade").unwrap();
    observer
        .batch_execute(
            "ALTER DATABASE storage_upgrade OWNER TO dc2_boundary; CREATE EXTENSION pgstattuple",
        )
        .unwrap();
    config.url = format!(
        "host='{}' port=5432 user=dc2_boundary dbname=storage_upgrade",
        cluster.socket_directory().display()
    );
    let database = Persistence::connect(&config).unwrap();
    // Install the actual immutable released SQL/checksums, not the candidate
    // migration. The extension is an observer-only fixture in this database;
    // the production role receives no extra privileges or extension grants.
    database.maintenance_transaction::<_,Error,_>(|tx| {
        tx.execute_batch("CREATE TABLE dc2_schema_migrations(version BIGINT PRIMARY KEY,name TEXT NOT NULL,sha256 TEXT NOT NULL,applied_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp())")?;
        for (version,name,sql) in MIGRATIONS.iter().filter(|(version,_,_)|*version<=13) {
            tx.execute_batch(sql)?;
            let checksum=Sha256::digest(sql.as_bytes()).iter().map(|byte|format!("{byte:02x}")).collect::<String>();
            tx.execute("INSERT INTO dc2_schema_migrations(version,name,sha256) VALUES($1,$2,$3)",params![version,name,checksum])?;
        }
        Ok(())
    }).unwrap();
    let pending = |observer: &mut postgres::Client| -> (i64, i64) {
        let row = observer.query_one("SELECT (SELECT pending_pages::bigint FROM pgstatginindex('public.storage_artifacts_ancestors_live'::regclass)),(SELECT pending_pages::bigint FROM pgstatginindex('public.storage_leases_artifacts'::regclass))", &[]).unwrap();
        (row.get(0), row.get(1))
    };
    let fingerprints =
        |connection: &devcoordinator2_persistence::Connection| -> Result<Vec<String>> {
            [
            "SELECT md5(coalesce(string_agg(to_jsonb(t)::text,'' ORDER BY artifact_id),'')) FROM storage_artifacts t",
            "SELECT md5(coalesce(string_agg(to_jsonb(t)::text,'' ORDER BY lease_id),'')) FROM storage_leases t",
            "SELECT md5(coalesce(string_agg(to_jsonb(t)::text,'' ORDER BY lease_id,artifact_id),'')) FROM dc2_storage_lease_artifacts t",
            "SELECT md5(coalesce(string_agg(to_jsonb(t)::text,'' ORDER BY repository_id),'')) FROM repositories t",
            "SELECT md5(coalesce(string_agg(to_jsonb(t)::text,'' ORDER BY worktree_id),'')) FROM worktrees t",
        ].into_iter().map(|sql|connection.query_row(sql,[],|row|row.get(0))).collect()
        };
    let insert = |connection: &devcoordinator2_persistence::Connection,
                  first: i64,
                  last: i64|
     -> Result<()> {
        connection.execute("INSERT INTO storage_artifacts(artifact_id,kind,revision,record_json,updated_at_ms) SELECT 'gin-artifact-'||g,'build_output',1,jsonb_build_object('resource_key','resource-'||g,'ancestor_keys',CASE WHEN g%10=0 THEN jsonb_build_array('ancestor-'||(g%11)) ELSE '[]'::jsonb END,'artifact',jsonb_build_object('name','artifact-'||g,'protected',g%17=0,'dependencies',jsonb_build_array('gin-artifact-'||(g-1))),'blockers','[]'::jsonb),1 FROM generate_series($1::bigint,$2::bigint) g",params![first,last])?;
        connection.execute("INSERT INTO storage_leases(lease_id,actor,lease_json,expires_at_ms) SELECT 'gin-lease-'||g,'fixture',jsonb_build_object('artifact_ids',CASE WHEN g%10=0 THEN jsonb_build_array('gin-artifact-'||g) ELSE '[]'::jsonb END,'expires_at_ms',100000),100000 FROM generate_series($1::bigint,$2::bigint) g",params![first,last])?;
        Ok(())
    };
    let before = database
        .transaction::<_, Error, _>(|tx| {
            tx.execute_batch(
                "LOCK TABLE storage_artifacts,storage_leases IN SHARE ROW EXCLUSIVE MODE",
            )?;
            insert(tx, 1, 3000)?;
            tx.execute_batch("INSERT INTO repositories(repository_id,root_path,display_name,registered_at,registered_by_uid,last_seen_at) VALUES('r-upgrade-bulk','/fixture/upgrade/bulk','Bulk','t','1000','t'),('r-upgrade-rare','/fixture/upgrade/rare','Rare','t','1000','t'); INSERT INTO worktrees(worktree_id,repository_id,worktree_path,registered_at,last_seen_at) SELECT 'w'||lpad(g::text,8,'0'),CASE WHEN g%1000=0 THEN 'r-upgrade-rare' ELSE 'r-upgrade-bulk' END,'/fixture/upgrade/worktree-'||g,'t','t' FROM generate_series(1,5000) g")?;
            let pages = pending(&mut observer);
            assert!(
                pages.0 > 0 && pages.1 > 0,
                "schema13 must contain real GIN pending pages before upgrade: {pages:?}"
            );
            fingerprints(tx)
        })
        .unwrap();
    database.migrate().unwrap();
    assert_eq!(pending(&mut observer), (0, 0));
    assert_eq!(
        database
            .read::<_, Error, _>(|connection| fingerprints(connection))
            .unwrap(),
        before,
        "stored JSON, generated safety inputs, derived leases and registry rows survive migration14 exactly"
    );
    database.transaction::<_,Error,_>(|tx| {
        tx.execute_batch("SET LOCAL plan_cache_mode=force_generic_plan")?;
        let query="SELECT w.worktree_id FROM worktrees w JOIN repositories r ON r.repository_id=w.repository_id WHERE r.archived_at IS NULL AND w.repository_id=$1 AND w.worktree_id>$2 ORDER BY w.worktree_id LIMIT $3";
        let plan:String=tx.query_row(&format!("EXPLAIN (FORMAT JSON) {query}"),params!["r-upgrade-rare","w00002000",2_i64],|row|row.get(0))?;
        let plan:serde_json::Value=serde_json::from_str(&plan).unwrap();
        assert!(plan.to_string().contains("\"Index Name\":\"worktrees_repository_seek\""),"the generic scoped query must seek its composite index");
        let rows=tx.prepare(query)?.query_map(params!["r-upgrade-rare","w00002000",2_i64],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>>>()?;
        assert_eq!(rows,["w00003000","w00004000"]);
        Ok(())
    }).unwrap();
    let indexes=observer.query("SELECT relname,reloptions FROM pg_class WHERE oid IN ('public.storage_artifacts_ancestors_live'::regclass,'public.storage_leases_artifacts'::regclass) ORDER BY relname",&[]).unwrap();
    assert_eq!(indexes.len(), 2);
    for index in indexes {
        assert!(
            index
                .get::<_, Vec<String>>(1)
                .iter()
                .any(|option| option == "fastupdate=off")
        );
    }
    let journal = || {
        database.read::<_,Error,_>(|c|c.query_row("SELECT md5(string_agg(to_jsonb(m)::text,'' ORDER BY version)) FROM dc2_schema_migrations m",[],|r|r.get::<_,String>(0))).unwrap()
    };
    let applied = journal();
    database.migrate().unwrap();
    assert_eq!(
        journal(),
        applied,
        "second native migration is a true no-op"
    );
    database
        .transaction::<_, Error, _>(|tx| insert(tx, 3001, 3200))
        .unwrap();
    assert_eq!(
        pending(&mut observer),
        (0, 0),
        "fresh inserts must not depend on later GIN cleanup"
    );
    assert_eq!(
        database
            .read::<_, Error, _>(|c| c.query_row(
                "SELECT count(*) FROM storage_artifacts",
                [],
                |r| r.get::<_, u64>(0)
            ))
            .unwrap(),
        3200
    );
}

#[test]
fn authority_schema_pools_timeouts_rollback_and_restart() {
    let cluster = FixtureCluster::start().expect("real PostgreSQL fixture");
    let mut config = cluster.create_database("boundary").unwrap();
    {
        let mut admin = cluster.client("postgres").unwrap();
        admin.batch_execute("CREATE ROLE dc2_boundary LOGIN CREATEDB NOSUPERUSER NOCREATEROLE NOREPLICATION; ALTER DATABASE boundary OWNER TO dc2_boundary").unwrap();
        admin
            .batch_execute("ALTER SYSTEM SET autovacuum_naptime='1s'")
            .unwrap();
        admin.batch_execute("SELECT pg_reload_conf()").unwrap();
        let role = admin.query_one("SELECT rolsuper,rolcreatedb,rolcreaterole,pg_has_role(oid,'pg_signal_backend','member') FROM pg_roles WHERE rolname='dc2_boundary'", &[]).unwrap();
        assert!(
            !role.get::<_, bool>(0)
                && role.get::<_, bool>(1)
                && !role.get::<_, bool>(2)
                && !role.get::<_, bool>(3)
        );
        assert_eq!(
            admin
                .query_one("SHOW autovacuum", &[])
                .unwrap()
                .get::<_, String>(0),
            "on"
        );
    }
    config.url = format!(
        "host='{}' port=5432 user=dc2_boundary dbname=boundary",
        cluster.socket_directory().display()
    );
    request_index_upgrade_preserves_native_rows(&cluster);
    let private = tempfile::tempdir().unwrap();
    let private_path = private.path().join("postgres.json");
    std::fs::write(&private_path,serde_json::to_vec(&serde_json::json!({"schema":1,"connection":config.url,"data_directory":config.data_directory,"pools":{"reads":2,"writes":1,"maintenance":1},"timeouts_ms":{"connection":500,"statement":2000,"lock":300,"idle_transaction":3000}})).unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&private_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let tuning = devcoordinator2_persistence::DatabaseConfig::from_file(&private_path).unwrap();
    assert_eq!(
        tuning.pool_sizes,
        PoolSizes {
            reads: 2,
            writes: 1,
            maintenance: 1
        }
    );
    assert_eq!(tuning.statement_timeout, Duration::from_millis(2000));
    assert_eq!(
        tuning.data_directory,
        config
            .data_directory
            .as_ref()
            .map(|p| p.canonicalize().unwrap())
    );
    std::fs::set_permissions(&private_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(devcoordinator2_persistence::DatabaseConfig::from_file(&private_path).is_err());
    let database = Persistence::connect(&config).unwrap();
    database
        .migrate()
        .unwrap_or_else(|e| panic!("native schema: {e:?}"));
    database
        .migrate()
        .expect("ordered migrations are idempotent");
    let readonly = database.read::<_, Error, _>(|c| {
        c.execute("INSERT INTO meta(key,value) VALUES('forbidden','read')", [])
    });
    assert!(
        matches!(readonly,Err(Error::Postgres(e)) if e.code()==Some(&postgres::error::SqlState::READ_ONLY_SQL_TRANSACTION))
    );
    let transport = database
        .read::<_, Error, _>(|c| {
            c.query_row(
                "SELECT inet_server_addr() IS NULL,current_setting('listen_addresses')",
                [],
                |r| Ok((r.get::<_, bool>(0)?, r.get::<_, String>(1)?)),
            )
        })
        .unwrap();
    assert_eq!(transport, (true, String::new()));
    for lane in [Lane::Read, Lane::Write, Lane::Maintenance] {
        let session = database.acquire(lane).unwrap();
        assert_eq!(
            session
                .query_row("SHOW jit", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "off"
        );
    }
    database.write::<_,Error,_>(|c|{
        let document=serde_json::json!({"spaces":"a  b ","escaped":"quote\" slash\\ tab\t","utf8":"é中","nested":[{},[],true,false,serde_json::Value::Null]});
        let compact=serde_json::to_string(&document).unwrap();
        let measured:u64=c.query_row("SELECT dc2_jsonb_compact_bytes($1::jsonb)",[&compact],|r|r.get(0))?;
        assert_eq!(measured,compact.len() as u64,"spaces and escapes inside strings count toward UTF-8 bytes");
        for (number,bytes) in [("1e10000",7_u64),("-1e10000",8),("0.0001",4),("1.2000",3),("0.0",1)]{
            assert_eq!(c.query_row("SELECT dc2_jsonb_compact_bytes($1::jsonb)",[number],|r|r.get::<_,u64>(0))?,bytes);
        }
        let within=serde_json::to_string(&"é".repeat(4095)).unwrap();
        assert_eq!(within.len(),8192);
        c.execute("INSERT INTO owned_events(occurred_at,category,kind,payload_json) VALUES('t','other','size-boundary',$1)",[within])?;
        let too_big=serde_json::to_string(&"é".repeat(4096)).unwrap();
        assert!(c.execute("INSERT INTO owned_events(occurred_at,category,kind,payload_json) VALUES('t','other','size-overflow',$1)",[too_big]).is_err());
        Ok(())
    }).unwrap();
    // pg_restore clears search_path. Store nested constrained JSON under that
    // same condition so the native archive below exercises recursive checks.
    database
        .transaction::<_, Error, _>(|tx| {
            tx.execute_batch("SET LOCAL search_path=''")?;
            tx.execute(
                "INSERT INTO public.owned_events(occurred_at,category,kind,payload_json) VALUES('t','other','nested-restore',$1)",
                [r#"{"nested":[{"number":1,"text":"a  b"},[true,null]]}"#],
            )?;
            Ok(())
        })
        .unwrap();
    database
        .transaction::<_, Error, _>(|tx| {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES($1,$2)",
                params!["durable", "kept"],
            )?;
            Ok(())
        })
        .unwrap();
    let rolled_back = database.transaction::<(), Error, _>(|tx| {
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('rollback','absent')",
            [],
        )?;
        Err(Error::InvalidQuery)
    });
    assert!(rolled_back.is_err());
    assert_eq!(
        database
            .read::<_, Error, _>(|c| c.query_row(
                "SELECT count(*) FROM meta WHERE key='rollback'",
                [],
                |r| r.get::<_, i64>(0)
            ))
            .unwrap(),
        0
    );
    database
        .write::<_, Error, _>(|c| {
            let values = c.query_row(
                "SELECT $1::bigint,$2::bigint,$3::jsonb,$4::bytea",
                params![1_u64, 0_u32, r#"{"z":[1,true],"a":null}"#, vec![0_u8, 255]],
                |r| {
                    Ok((
                        r.get::<_, bool>(0)?,
                        r.get::<_, u64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )?;
            assert!(values.0);
            assert_eq!(values.1, 0);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&values.2).unwrap(),
                serde_json::json!({"z":[1,true],"a":null})
            );
            assert_eq!(values.3, [0, 255]);
            assert_eq!(
                c.query_row("SELECT $1::numeric::text", params![u64::MAX], |r| r
                    .get::<_, String>(0))?,
                u64::MAX.to_string()
            );
            assert!(matches!(
                c.query_row("SELECT -1::bigint", [], |r| r.get::<_, u64>(0)),
                Err(Error::IntegralValueOutOfRange(0, -1))
            ));
            assert!(
                c.query_row("SELECT $1::bigint", params![u64::MAX], |r| r
                    .get::<_, i64>(0))
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    let mut bounded = config.clone();
    bounded.pool_sizes = PoolSizes {
        reads: 1,
        writes: 1,
        maintenance: 1,
    };
    bounded.connection_timeout = Duration::from_millis(180);
    bounded.statement_timeout = Duration::from_millis(90);
    bounded.lock_timeout = Duration::from_millis(45);
    let bounded = Persistence::connect(&bounded).unwrap();
    let start = Instant::now();
    let timeout =
        bounded.write::<_, Error, _>(|c| c.query_row("SELECT pg_sleep(1)", [], |_| Ok(())));
    assert!(timeout.as_ref().err().is_some_and(Error::is_timeout));
    assert!(start.elapsed() < Duration::from_secs(1));
    // SET SESSION also bounds statements inside a real transaction.
    let timeout =
        bounded.transaction::<_, Error, _>(|tx| tx.query_row("SELECT pg_sleep(1)", [], |_| Ok(())));
    assert!(timeout.as_ref().err().is_some_and(Error::is_timeout));
    let maintenance = bounded.acquire(Lane::Maintenance).unwrap();
    assert_eq!(
        bounded
            .read::<_, Error, _>(|c| c.query_row("SELECT 7::bigint", [], |r| r.get::<_, i64>(0)))
            .unwrap(),
        7
    );
    let saturation = bounded.maintenance::<_, Error, _>(|_| Ok(()));
    assert!(matches!(saturation, Err(Error::Pool(_))));
    drop(maintenance);
    // A held row lock must respect the lock deadline rather than occupy a
    // request worker indefinitely. The other connection retains ownership.
    let owner = database.acquire(Lane::Write).unwrap();
    let held = owner.unchecked_transaction().unwrap();
    held.execute("UPDATE meta SET value='locked' WHERE key='durable'", [])
        .unwrap();
    let lock = bounded.write::<_, Error, _>(|c| {
        c.execute("UPDATE meta SET value='blocked' WHERE key='durable'", [])
    });
    assert!(lock.as_ref().err().is_some_and(Error::is_timeout));
    held.rollback().unwrap();
    drop(owner);
    drop(bounded);
    cluster.restart().unwrap();
    assert_eq!(
        database
            .read::<_, Error, _>(|c| c.query_row(
                "SELECT value FROM meta WHERE key='durable'",
                [],
                |r| r.get::<_, String>(0)
            ))
            .unwrap(),
        "kept"
    );
    // Native archive restore reproduces data and invariants after a restart.
    let directory = tempfile::tempdir().unwrap();
    let archive = directory.path().join("authority.pgdump");
    backup::dump(&config, &archive).unwrap();
    let archive_receipt = backup::verify_archive(&config, &archive).unwrap();
    assert!(archive_receipt.verified && archive_receipt.isolated_restore_removed);
    assert!(
        archive_receipt
            .tables
            .iter()
            .any(|table| table.table == "meta")
    );
    let meta_head = archive_receipt
        .sequences
        .iter()
        .find(|s| s.sequence == "meta_rowid_seq")
        .unwrap();
    database
        .write::<_, Error, _>(|c| {
            c.query_row("SELECT nextval('meta_rowid_seq')", [], |r| {
                r.get::<_, i64>(0)
            })
        })
        .unwrap();
    let sequence_difference = backup::compare_archive(&config, &archive).unwrap();
    assert!(!sequence_difference.verification.verified);
    assert!(!sequence_difference.verification.matched_frozen_authority);
    assert!(sequence_difference.verification.isolated_restore_removed);
    let changed = sequence_difference.comparison.unwrap();
    assert_eq!(changed.changed_table_count, 0);
    assert_eq!(changed.changed_sequence_count, 1);
    assert_eq!(changed.sequences[0].sequence, "meta_rowid_seq");
    assert!(!changed.sequences[0].value_equal);
    assert!(backup::verify_archive(&config, &archive).is_err());
    database
        .write::<_, Error, _>(|c| {
            c.query_row(
                "SELECT setval('meta_rowid_seq',$1,$2)",
                params![meta_head.value, meta_head.called],
                |r| r.get::<_, i64>(0),
            )
        })
        .unwrap();
    let restored = cluster.create_database("restored").unwrap();
    backup::restore(&restored, &archive).unwrap();
    let restored = Persistence::connect(&restored).unwrap();
    restored.migrate().unwrap();
    assert_eq!(
        restored
            .read::<_, Error, _>(|c| c.query_row(
                "SELECT value FROM meta WHERE key='durable'",
                [],
                |r| r.get::<_, String>(0)
            ))
            .unwrap(),
        "kept"
    );
    let mut snapshot_name = String::new();
    let inspected = backup::with_snapshot::<_, Error, _>(&config, &archive, |connection| {
        snapshot_name = connection.query_row("SELECT current_database()", [], |r| r.get(0))?;
        assert!(
            connection
                .execute(
                    "INSERT INTO meta(key,value) VALUES('forbidden','snapshot')",
                    []
                )
                .is_err()
        );
        // A restore large enough for autovacuum exposed the production-role
        // failure. Hold a real worker active, with no signalling privileges.
        let writer = Persistence::connect(&config.for_database(&snapshot_name))?;
        writer.write::<_,Error,_>(|client| {
            assert_eq!(client.query_row("SELECT current_database()",[],|row|row.get::<_,String>(0))?,snapshot_name);
            client.execute_batch("CREATE TABLE cleanup_pressure(id bigint,payload text) WITH(autovacuum_vacuum_threshold=0,autovacuum_vacuum_scale_factor=0,autovacuum_vacuum_cost_delay=100,autovacuum_vacuum_cost_limit=1); INSERT INTO cleanup_pressure SELECT n,repeat('x',200) FROM generate_series(1,100000) n; DELETE FROM cleanup_pressure WHERE id%2=0")
        })?;
        drop(writer);
        assert!(database.read::<_,Error,_>(|client|client.query_row("SELECT to_regclass('public.cleanup_pressure') IS NULL",[],|row|row.get::<_,bool>(0)))?,"pressure must never be written into the authority database");
        let mut admin = cluster.client("postgres")?;
        let started = Instant::now();
        loop {
            let active:bool=admin.query_one("SELECT EXISTS(SELECT 1 FROM pg_stat_progress_vacuum v JOIN pg_stat_activity a USING(pid) WHERE a.datname=$1 AND a.backend_type='autovacuum worker')", &[&snapshot_name])?.get(0);
            if active { break; }
            if started.elapsed()>=Duration::from_secs(30) {
                let settings=admin.query_one("SELECT current_setting('autovacuum'),current_setting('autovacuum_naptime'),current_setting('autovacuum_max_workers'),current_setting('track_counts')",&[]).ok().map(|row|(row.get::<_,String>(0),row.get::<_,String>(1),row.get::<_,String>(2),row.get::<_,String>(3)));
                let workers=admin.query("SELECT backend_type,coalesce(state,''),coalesce(wait_event_type,''),COUNT(*)::bigint FROM pg_stat_activity WHERE backend_type IN ('autovacuum launcher','autovacuum worker') GROUP BY backend_type,state,wait_event_type LIMIT 16",&[]).ok().map(|rows|rows.into_iter().map(|row|(row.get::<_,String>(0),row.get::<_,String>(1),row.get::<_,String>(2),row.get::<_,i64>(3))).collect::<Vec<_>>());
                let pressure=cluster.client(&snapshot_name).ok().and_then(|mut client|client.query_opt("SELECT n_live_tup,n_dead_tup,autovacuum_count,autoanalyze_count FROM pg_stat_all_tables WHERE schemaname='public' AND relname='cleanup_pressure'",&[]).ok().flatten()).map(|row|(row.get::<_,i64>(0),row.get::<_,i64>(1),row.get::<_,i64>(2),row.get::<_,i64>(3)));
                eprintln!("autovacuum_readiness_failure {}",serde_json::json!({"elapsed_ms":started.elapsed().as_millis(),"settings":settings,"workers":workers,"pressure_stats":pressure}));
                panic!("real autovacuum did not start");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut owner = config.url.parse::<postgres::Config>()?.connect(postgres::NoTls)?;
        let old_cleanup = owner.batch_execute(&format!("DROP DATABASE \"{snapshot_name}\" WITH(FORCE)"));
        assert!(matches!(old_cleanup, Err(ref error) if error.code()==Some(&postgres::error::SqlState::INSUFFICIENT_PRIVILEGE)), "FORCE no longer reproduces the restricted-role autovacuum failure");
        drop(owner);
        drop(admin);
        connection.query_row("SELECT value FROM meta WHERE key='durable'", [], |r| {
            r.get::<_, String>(0)
        })
    })
    .unwrap();
    assert_eq!(inspected, "kept");
    assert!(
        cluster
            .client("postgres")
            .unwrap()
            .query_opt(
                "SELECT 1 FROM pg_database WHERE datname=$1",
                &[&snapshot_name]
            )
            .unwrap()
            .is_none()
    );
    // Snapshot reads must honor the caller's statement budget, even though
    // restoration itself uses the explicit offline archive boundary.
    let mut short_inspection = config.clone();
    short_inspection.statement_timeout = Duration::from_millis(90);
    let timeout =
        backup::with_snapshot::<(), Error, _>(&short_inspection, &archive, |connection| {
            assert_eq!(
                connection
                    .query_row("SHOW statement_timeout", [], |row| row.get::<_, String>(0))?,
                "90ms"
            );
            connection.query_row("SELECT pg_sleep(1)", [], |_| Ok(()))
        });
    assert!(timeout.as_ref().err().is_some_and(Error::is_timeout));
    let mut failed_name = String::new();
    let failure = backup::with_snapshot::<(), Error, _>(&config, &archive, |connection| {
        failed_name = connection.query_row("SELECT current_database()", [], |r| r.get(0))?;
        Err(Error::InvalidQuery)
    });
    assert!(matches!(failure, Err(Error::InvalidQuery)));
    assert!(
        cluster
            .client("postgres")
            .unwrap()
            .query_opt(
                "SELECT 1 FROM pg_database WHERE datname=$1",
                &[&failed_name]
            )
            .unwrap()
            .is_none()
    );
    let mut blocked_name = String::new();
    let mut blocker = config
        .url
        .parse::<postgres::Config>()
        .unwrap()
        .connect(postgres::NoTls)
        .unwrap();
    let cleanup_started = Instant::now();
    let failure = backup::with_snapshot::<(), Error, _>(&config, &archive, |connection| {
        blocked_name = connection.query_row("SELECT current_database()", [], |r| r.get(0))?;
        blocker.batch_execute("BEGIN")?;
        // COMMENT retains an object lock until rollback. This is
        // an exact-owned lock conflict, not a session the helper may kill.
        blocker.batch_execute(&format!(
            "COMMENT ON DATABASE \"{blocked_name}\" IS 'cleanup lock fixture'"
        ))?;
        Err(Error::Migration("preserved callback failure".into()))
    });
    assert!(
        cleanup_started.elapsed() < Duration::from_secs(20),
        "cleanup lock deadline was not enforced"
    );
    assert!(
        matches!(&failure,Err(Error::SnapshotCleanup {primary,cleanup}) if primary.contains("preserved callback failure") && cleanup.is_timeout())
    );
    assert!(failure.unwrap_err().is_timeout());
    blocker.batch_execute("ROLLBACK").unwrap();
    assert!(
        cluster
            .client("postgres")
            .unwrap()
            .query_opt(
                "SELECT 1 FROM pg_database WHERE datname=$1",
                &[&blocked_name]
            )
            .unwrap()
            .is_some(),
        "failed cleanup claimed the database was removed"
    );
    blocker
        .batch_execute(&format!("DROP DATABASE \"{blocked_name}\""))
        .unwrap();
    drop(blocker);
    assert!(
        cluster
            .client("postgres")
            .unwrap()
            .query_opt(
                "SELECT 1 FROM pg_database WHERE datname=$1",
                &[&blocked_name]
            )
            .unwrap()
            .is_none()
    );
    let no_tcp =
        devcoordinator2_persistence::DatabaseConfig::new("host=127.0.0.1 dbname=forbidden");
    assert!(matches!(
        Persistence::connect(&no_tcp),
        Err(Error::Configuration(_))
    ));
    database
        .write::<_, Error, _>(|c| {
            c.execute(
                "INSERT INTO meta(key,value) VALUES('after-dump','live-write')",
                [],
            )
        })
        .unwrap();
    assert!(backup::verify_archive(&config, &archive).is_err());
    let table_difference = backup::compare_archive(&config, &archive).unwrap();
    assert!(!table_difference.verification.verified);
    let changed = table_difference.comparison.as_ref().unwrap();
    let meta = changed.tables.iter().find(|t| t.table == "meta").unwrap();
    assert!(meta.columns_equal && meta.source_present && meta.restored_present);
    assert!(!meta.rows_equal && !meta.hash_equal);
    let private_report = serde_json::to_string(&table_difference).unwrap();
    assert!(!private_report.contains("live-write") && !private_report.contains("after-dump"));
    let recoverable = backup::verify_recoverable_archive(&config, &archive).unwrap();
    assert!(
        recoverable.verified
            && recoverable.isolated_restore_removed
            && !recoverable.matched_frozen_authority
    );
    let _: Result<()> = Ok(());
    cluster.stop().expect("owned fixture shutdown");
}
