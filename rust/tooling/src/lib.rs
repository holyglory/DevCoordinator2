use std::path::Path;

pub mod python_guard;
pub mod repository_checks;
pub mod skill_links;

pub fn export_contract(path: &Path, check: bool) -> Result<(), String> {
    let document = devcoordinator2_api::contract_document();
    let mut rendered =
        serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?;
    rendered.push('\n');
    if check {
        let current = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if current != rendered {
            return Err(format!(
                "{} does not match the Rust contract",
                path.display()
            ));
        }
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(path, rendered)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}
