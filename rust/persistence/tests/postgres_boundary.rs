//! Real database evidence. Every test runs against the owned socket-only
//! fixture runtime, never the installed Coordinator or another agent's DB.

use devcoordinator2_persistence::{
    Error, Lane, Persistence, PoolSizes, Result, backup, fixture::FixtureCluster, params,
};
use std::time::{Duration, Instant};

#[test]
fn authority_schema_pools_timeouts_rollback_and_restart() {
    let cluster = FixtureCluster::start().expect("real PostgreSQL fixture");
    let config = cluster.create_database("boundary").unwrap();
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
    let mut failed_name = String::new();
    let failure = backup::with_snapshot::<(), Error, _>(&config, &archive, |connection| {
        failed_name = connection.query_row("SELECT current_database()", [], |r| r.get(0))?;
        Err(Error::InvalidQuery)
    });
    assert!(failure.is_err());
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
    let recoverable = backup::verify_recoverable_archive(&config, &archive).unwrap();
    assert!(
        recoverable.verified
            && recoverable.isolated_restore_removed
            && !recoverable.matched_frozen_authority
    );
    let _: Result<()> = Ok(());
    cluster.stop().expect("owned fixture shutdown");
}
