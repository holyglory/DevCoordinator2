//! Read-only, secret-free export of import-eligible legacy state.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use rustix::fs::{self as unix_fs, Dir, Mode, OFlags};
use serde_json::{Map, Value, json};

const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BUG_BYTES: u64 = 64 * 1024;
const SECRET_HINTS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "credential",
    "key",
    "authorization",
];

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub authority_db: PathBuf,
    pub routes_publication: Option<PathBuf>,
    pub access_control: Option<PathBuf>,
    pub telegram_state: Option<PathBuf>,
    pub bugs_dir: Option<PathBuf>,
    pub output: PathBuf,
}

pub fn export_document(options: &ExportOptions, exported_at: &str) -> Result<Value, String> {
    let mut document = Map::new();
    document.insert("exported_at".to_owned(), json!(exported_at));
    document.insert(
        "authority".to_owned(),
        export_authority(&options.authority_db)?,
    );
    for (name, path, exporter) in [
        (
            "routes",
            options.routes_publication.as_deref(),
            export_routes as fn(&Path) -> Result<Value, String>,
        ),
        (
            "access_control",
            options.access_control.as_deref(),
            export_access_control,
        ),
        (
            "telegram",
            options.telegram_state.as_deref(),
            export_telegram,
        ),
    ] {
        if let Some(path) = path.filter(|path| path.exists()) {
            document.insert(name.to_owned(), exporter(path)?);
        }
    }
    if let Some(directory) = &options.bugs_dir {
        document.insert("open_bugs".to_owned(), export_bugs(directory)?);
    }
    Ok(Value::Object(document))
}

pub fn write_export(options: &ExportOptions, exported_at: &str) -> Result<Value, String> {
    let document = export_document(options, exported_at)?;
    let mut payload = serde_json::to_vec_pretty(&document)
        .map_err(|error| format!("cannot encode legacy export: {error}"))?;
    payload.push(b'\n');
    atomic_write(&options.output, &payload)?;
    Ok(json!({
        "out": options.output,
        "summary": summary(&document),
    }))
}

pub fn export_authority(path: &Path) -> Result<Value, String> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )
    .map_err(|error| format!("cannot open legacy authority database read-only: {error}"))?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| format!("cannot protect legacy authority database: {error}"))?;

    let repositories = query_repositories(&connection)?;
    let by_id = repositories
        .iter()
        .map(|repository| (repository.id.clone(), repository.clone()))
        .collect::<HashMap<_, _>>();
    let active = repositories
        .iter()
        .filter(|repository| repository.state == "active")
        .map(|repository| repository.id.clone())
        .collect::<HashSet<_>>();
    let installations = query_installations(&connection)?;

    let repository_values = repositories
        .into_iter()
        .map(|repository| {
            json!({
                "legacy_repo_id": repository.id,
                "root": repository.root,
                "display_name": repository.display_name,
                "state": repository.state,
                "installation": installations.get(&repository.id).cloned().flatten(),
            })
        })
        .collect::<Vec<_>>();

    let mut ports = Vec::new();
    let mut statement = connection
        .prepare("SELECT repo_id,server_name,port,status FROM port_assignments")
        .map_err(sql_error)?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(sql_error)?
    {
        let (repository_id, server, port, status) = row.map_err(sql_error)?;
        ports.push(json!({
            "root": by_id.get(&repository_id).map(|repository| repository.root.clone()),
            "server": server,
            "port": port,
            "status": status,
        }));
    }
    drop(statement);

    let mut servers = Vec::new();
    let mut statement = connection
        .prepare(
            "SELECT server_definition_id,repo_id,name,role,cwd,health_url_template,log_path FROM server_definitions",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ServerRow {
                id: row.get(0)?,
                repository_id: row.get(1)?,
                name: row.get(2)?,
                role: row.get(3)?,
                cwd: row.get(4)?,
                health_url_template: row.get(5)?,
                log_path: row.get(6)?,
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    drop(statement);
    for server in rows {
        if !active.contains(&server.repository_id) {
            continue;
        }
        let command = query_strings(
            &connection,
            "SELECT argument FROM server_command_arguments WHERE server_definition_id=?1 ORDER BY ordinal",
            &server.id,
        )?;
        let environment_names = query_strings(
            &connection,
            "SELECT name FROM server_environment WHERE server_definition_id=?1",
            &server.id,
        )?;
        let environment_looks_secret = environment_names
            .iter()
            .filter(|name| {
                let lower = name.to_lowercase();
                SECRET_HINTS.iter().any(|hint| lower.contains(hint))
            })
            .cloned()
            .collect::<Vec<_>>();
        let root = by_id
            .get(&server.repository_id)
            .map(|repository| repository.root.clone());
        servers.push(json!({
            "root": root,
            "name": server.name,
            "role": server.role,
            "cwd": server.cwd,
            "command": command,
            "health_url_template": server.health_url_template,
            "log_path": server.log_path,
            "environment_names": environment_names,
            "environment_looks_secret": environment_looks_secret,
        }));
    }

    let mut docker_resources = Vec::new();
    let mut statement = connection
        .prepare(
            "SELECT docker_resource_id,full_container_id,current_name,image,repo_id FROM docker_resources WHERE repo_id IS NOT NULL",
        )
        .map_err(sql_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    drop(statement);
    for (id, container_id, name, image, repository_id) in rows {
        let mut labels = HashMap::new();
        let mut labels_statement = connection
            .prepare("SELECT name,value FROM docker_labels WHERE docker_resource_id=?1")
            .map_err(sql_error)?;
        for label in labels_statement
            .query_map([&id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(sql_error)?
        {
            let (name, value) = label.map_err(sql_error)?;
            labels.insert(name, value);
        }
        docker_resources.push(json!({
            "root": by_id.get(&repository_id).map(|repository| repository.root.clone()),
            "container_id": container_id,
            "name": name,
            "image": image,
            "compose_project": labels.get("com.docker.compose.project"),
            "compose_service": labels.get("com.docker.compose.service"),
        }));
    }

    let mut database_bindings = Vec::new();
    let mut statement = connection
        .prepare("SELECT repo_id,database_name,engine_kind FROM database_bindings")
        .map_err(sql_error)?;
    for row in statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(sql_error)?
    {
        let (repository_id, database, engine) = row.map_err(sql_error)?;
        database_bindings.push(json!({
            "root": by_id.get(&repository_id).map(|repository| repository.root.clone()),
            "database": database,
            "engine": engine,
        }));
    }

    Ok(json!({
        "repositories": repository_values,
        "port_assignments": ports,
        "server_definitions": servers,
        "docker_resources": docker_resources,
        "database_bindings": database_bindings,
    }))
}

pub fn export_routes(path: &Path) -> Result<Value, String> {
    let document = read_json(path, MAX_JSON_BYTES)?;
    let publication = document.get("publication").unwrap_or(&document);
    let empty_routes = Map::new();
    let routes = match publication.get("routes") {
        None | Some(Value::Null) => &empty_routes,
        Some(Value::Object(routes)) => routes,
        Some(_) => return Err("legacy route publication routes must be an object".to_owned()),
    };
    let mut projected = Vec::new();
    for (slug, route) in routes {
        let route = route
            .as_object()
            .ok_or_else(|| "legacy route must be an object".to_owned())?;
        let upstream = route
            .get("upstream")
            .filter(|value| !value.is_null())
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        projected.push(json!({
            "slug": slug,
            "auth": route.get("auth").cloned().unwrap_or(Value::Null),
            "kind": route.get("kind").cloned().unwrap_or(Value::Null),
            "upstream_host": upstream.get("host").cloned().unwrap_or(Value::Null),
            "upstream_port": upstream.get("port").cloned().unwrap_or(Value::Null),
            "upstream_status": upstream.get("status").cloned().unwrap_or_else(|| json!("configured")),
            "has_upstream_authorization": truthy(route.get("upstream_authorization"))
                || truthy(upstream.get("authorization")),
        }));
    }
    let access = publication
        .get("access")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "generation": publication.get("generation").cloned().unwrap_or(Value::Null),
        "domain": publication.get("domain").cloned().unwrap_or(Value::Null),
        "console_host": publication.get("console_host").cloned().unwrap_or(Value::Null),
        "routes": projected,
        "owners": access.get("owners").cloned().unwrap_or_else(|| json!([])),
        "grants": access.get("grants").cloned().unwrap_or_else(|| json!({})),
    }))
}

pub fn export_access_control(path: &Path) -> Result<Value, String> {
    let document = read_json(path, MAX_JSON_BYTES)?;
    let users = document
        .get("users")
        .filter(|value| truthy(Some(value)))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let users = match users {
        Value::Object(values) => Value::Array(
            values
                .into_iter()
                .map(|(key, _)| Value::String(key))
                .collect(),
        ),
        other => other,
    };
    let requests = ["requests", "access_requests", "pending"]
        .iter()
        .find_map(|name| document.get(*name).filter(|value| truthy(Some(value))))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let requests = match requests {
        Value::Object(values) => values.into_values().collect::<Vec<_>>(),
        Value::Array(values) => values,
        _ => Vec::new(),
    };
    let pending = requests
        .into_iter()
        .map(|request| {
            let mut selected = Map::new();
            if let Value::Object(request) = request {
                for name in ["email", "requested_at", "resource", "status"] {
                    if let Some(value) = request.get(name) {
                        selected.insert(name.to_owned(), value.clone());
                    }
                }
            }
            Value::Object(selected)
        })
        .collect::<Vec<_>>();
    Ok(json!({"users": users, "pending_requests": pending}))
}

pub fn export_telegram(path: &Path) -> Result<Value, String> {
    let document = read_json(path, MAX_JSON_BYTES)?;
    let bots = match document.get("bots") {
        Some(Value::Object(values)) => values.values().cloned().collect::<Vec<_>>(),
        Some(Value::Array(values)) => values.clone(),
        _ => Vec::new(),
    };
    let bots = bots
        .into_iter()
        .filter_map(|bot| bot.as_object().cloned())
        .map(|bot| {
            json!({
                "label": bot.get("label").cloned().unwrap_or(Value::Null),
                "username": bot.get("username").cloned().unwrap_or(Value::Null),
                "owner": bot.get("ownerEmail").cloned().unwrap_or(Value::Null),
                "enabled": bot.get("enabled").cloned().unwrap_or(Value::Null),
                "projects": bot.get("projects").cloned().unwrap_or_else(|| json!([])),
                "token": "<redacted>",
            })
        })
        .collect::<Vec<_>>();
    let authorizations = match document.get("authorizationRequests") {
        Some(Value::Object(values)) => values.values().cloned().collect::<Vec<_>>(),
        Some(Value::Array(values)) => values.clone(),
        _ => Vec::new(),
    }
    .into_iter()
    .filter_map(|authorization| authorization.as_object().cloned())
    .map(|authorization| {
        json!({
            "chat_id": authorization.get("chatId").filter(|value| truthy(Some(value))).cloned()
                .or_else(|| authorization.get("chat_id").cloned()).unwrap_or(Value::Null),
            "username": authorization.get("username").cloned().unwrap_or(Value::Null),
            "status": authorization.get("status").cloned().unwrap_or(Value::Null),
            "approved_by": authorization.get("approvedBy").cloned().unwrap_or(Value::Null),
        })
    })
    .collect::<Vec<_>>();
    let outbox_pending = document
        .get("outbox")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter(|message| message.get("status").and_then(Value::as_str) != Some("delivered"))
        .count();
    Ok(json!({
        "bots": bots,
        "authorizations": authorizations,
        "outbox_pending": outbox_pending,
    }))
}

pub fn export_bugs(path: &Path) -> Result<Value, String> {
    if !path.is_dir() || path.is_symlink() {
        return Ok(json!([]));
    }
    let directory = unix_fs::open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open legacy bug directory: {error}"))?;
    let mut names = Vec::new();
    let mut entries = Dir::read_from(&directory)
        .map_err(|error| format!("cannot list legacy bug directory: {error}"))?;
    for entry in &mut entries {
        let Ok(entry) = entry else { continue };
        let Ok(name) = entry.file_name().to_str() else {
            continue;
        };
        if name.ends_with(".json") {
            names.push(name.to_owned());
        }
    }
    names.sort();
    let mut bugs = Vec::new();
    for name in names {
        let Ok(file) = unix_fs::openat(
            &directory,
            &name,
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > MAX_BUG_BYTES {
            continue;
        }
        let mut raw = Vec::new();
        if file.take(MAX_BUG_BYTES + 1).read_to_end(&mut raw).is_ok()
            && raw.len() as u64 <= MAX_BUG_BYTES
            && let Ok(value) = serde_json::from_slice(&raw)
        {
            bugs.push(value);
        }
    }
    Ok(Value::Array(bugs))
}

#[derive(Clone)]
struct RepositoryRow {
    id: String,
    root: String,
    display_name: String,
    state: String,
}

struct ServerRow {
    id: String,
    repository_id: String,
    name: String,
    role: String,
    cwd: Option<String>,
    health_url_template: Option<String>,
    log_path: Option<String>,
}

fn query_repositories(connection: &Connection) -> Result<Vec<RepositoryRow>, String> {
    let mut statement = connection
        .prepare("SELECT repo_id,canonical_root,display_name,state FROM repositories")
        .map_err(sql_error)?;
    statement
        .query_map([], |row| {
            Ok(RepositoryRow {
                id: row.get(0)?,
                root: row.get(1)?,
                display_name: row.get(2)?,
                state: row.get(3)?,
            })
        })
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)
}

fn query_installations(connection: &Connection) -> Result<HashMap<String, Option<String>>, String> {
    let mut statement = connection
        .prepare("SELECT repo_id,status FROM repository_installations")
        .map_err(sql_error)?;
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(sql_error)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(sql_error)
}

fn query_strings(connection: &Connection, sql: &str, key: &str) -> Result<Vec<String>, String> {
    let mut statement = connection.prepare(sql).map_err(sql_error)?;
    statement
        .query_map([key], |row| row.get(0))
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)
}

fn read_json(path: &Path, maximum: u64) -> Result<Value, String> {
    let file = unix_fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| format!("cannot open legacy JSON input: {error}"))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("cannot inspect legacy JSON input: {error}"))?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err("legacy JSON input is not a bounded regular file".to_owned());
    }
    let mut raw = Vec::new();
    file.take(maximum + 1)
        .read_to_end(&mut raw)
        .map_err(|error| format!("cannot read legacy JSON input: {error}"))?;
    if raw.len() as u64 > maximum {
        return Err("legacy JSON input exceeds its size limit".to_owned());
    }
    serde_json::from_slice(&raw).map_err(|error| format!("legacy JSON input is invalid: {error}"))
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(value)) if value.is_empty() => false,
        Some(Value::Array(value)) if value.is_empty() => false,
        Some(Value::Object(value)) if value.is_empty() => false,
        Some(Value::Number(value)) if value.as_i64() == Some(0) => false,
        Some(_) => true,
    }
}

fn summary(document: &Value) -> Value {
    let mut output = Map::new();
    let Some(document) = document.as_object() else {
        return Value::Object(output);
    };
    for (name, value) in document {
        if name == "exported_at" {
            continue;
        }
        match value {
            Value::Array(values) => {
                output.insert(name.clone(), json!(values.len()));
            }
            Value::Object(values) => {
                let counts = values
                    .iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_array()
                            .map(|values| (key.clone(), json!(values.len())))
                    })
                    .collect::<Map<_, _>>();
                output.insert(name.clone(), Value::Object(counts));
            }
            _ => {}
        }
    }
    Value::Object(output)
}

fn sql_error(error: rusqlite::Error) -> String {
    format!("legacy authority query failed: {error}")
}

fn atomic_write(path: &Path, payload: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "legacy export path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create export directory: {error}"))?;
    let mut random = [0u8; 6];
    getrandom::fill(&mut random)
        .map_err(|error| format!("cannot create export temporary identity: {error}"))?;
    let temporary = parent.join(format!(".legacy-export-{}.tmp", hex(&random)));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("cannot create export temporary: {error}"))?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot protect legacy export: {error}"))?;
        file.write_all(payload)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("cannot persist legacy export: {error}"))?;
        std::fs::rename(&temporary, path)
            .map_err(|error| format!("cannot replace legacy export: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("cannot sync legacy export directory: {error}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(value, "{byte:02x}").expect("string write");
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_access_telegram_and_bug_exports_redact_private_values() {
        let temporary = tempfile::tempdir().unwrap();
        let routes = temporary.path().join("routes.json");
        std::fs::write(
            &routes,
            serde_json::to_vec(&json!({"publication": {
                "generation": 7,
                "domain": "example.test",
                "console_host": "console.example.test",
                "routes": {"web": {"auth":"public","kind":"http","upstream": {
                    "host":"127.0.0.1","port":3000,"authorization":"secret"
                }}},
                "access": {"owners":["owner@example.test"],"grants":{}}
            }}))
            .unwrap(),
        )
        .unwrap();
        let projected = export_routes(&routes).unwrap();
        assert_eq!(projected["routes"][0]["slug"], "web");
        assert_eq!(projected["routes"][0]["has_upstream_authorization"], true);
        assert!(!projected.to_string().contains("secret"));

        let telegram = temporary.path().join("telegram.json");
        std::fs::write(
            &telegram,
            serde_json::to_vec(&json!({
                "bots":{"one":{"label":"one","username":"bot","ownerEmail":"owner@example.test","enabled":true,"projects":[],"token":"private"}},
                "authorizationRequests":{"a":{"chatId":42,"status":"approved"}},
                "outbox":[{"status":"pending"},{"status":"delivered"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let projected = export_telegram(&telegram).unwrap();
        assert_eq!(projected["bots"][0]["token"], "<redacted>");
        assert_eq!(projected["outbox_pending"], 1);
        assert!(!projected.to_string().contains("private"));

        let bugs = temporary.path().join("bugs");
        std::fs::create_dir(&bugs).unwrap();
        std::fs::write(bugs.join("b1.json"), br#"{"summary":"one"}"#).unwrap();
        std::fs::write(bugs.join("bad.json"), b"not-json").unwrap();
        assert_eq!(export_bugs(&bugs).unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn authority_export_keeps_names_but_never_environment_values() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("legacy.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE repositories(repo_id TEXT,canonical_root TEXT,display_name TEXT,state TEXT);
                 CREATE TABLE repository_installations(repo_id TEXT,status TEXT,startup_fenced INTEGER);
                 CREATE TABLE port_assignments(repo_id TEXT,server_name TEXT,port INTEGER,status TEXT);
                 CREATE TABLE server_definitions(server_definition_id TEXT,repo_id TEXT,name TEXT,role TEXT,cwd TEXT,health_url_template TEXT,log_path TEXT);
                 CREATE TABLE server_command_arguments(server_definition_id TEXT,ordinal INTEGER,argument TEXT);
                 CREATE TABLE server_environment(server_definition_id TEXT,name TEXT,value TEXT);
                 CREATE TABLE docker_resources(docker_resource_id TEXT,full_container_id TEXT,current_name TEXT,image TEXT,repo_id TEXT);
                 CREATE TABLE docker_labels(docker_resource_id TEXT,name TEXT,value TEXT);
                 CREATE TABLE database_bindings(database_binding_id TEXT,repo_id TEXT,database_name TEXT,engine_kind TEXT);
                 INSERT INTO repositories VALUES('L1','/repo','repo','active');
                 INSERT INTO repository_installations VALUES('L1','installed',0);
                 INSERT INTO port_assignments VALUES('L1','web',3000,'active');
                 INSERT INTO server_definitions VALUES('s1','L1','web','web','.','http://x','/log');
                 INSERT INTO server_command_arguments VALUES('s1',0,'node');
                 INSERT INTO server_environment VALUES('s1','API_TOKEN','never-export-this');",
            )
            .unwrap();
        drop(connection);
        let projected = export_authority(&path).unwrap();
        assert_eq!(
            projected["server_definitions"][0]["command"],
            json!(["node"])
        );
        assert_eq!(
            projected["server_definitions"][0]["environment_looks_secret"],
            json!(["API_TOKEN"])
        );
        assert!(!projected.to_string().contains("never-export-this"));
    }
}
