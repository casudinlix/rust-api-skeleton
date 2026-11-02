use crate::database::DatabasePool;

/// Default project seeder. The generic skeleton intentionally inserts no users,
/// credentials, roles, or domain data. Downstream projects should call their
/// registered seeders explicitly from here when a default seed set is desired.
pub async fn run(_database: &DatabasePool) -> anyhow::Result<()> {
    Ok(())
}
