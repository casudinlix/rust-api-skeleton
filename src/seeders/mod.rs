pub mod database_seeder;

use anyhow::bail;

use crate::database::DatabasePool;

/// Run a registered seeder class. Keep this registry explicit so seeds remain
/// reviewable and cannot be selected through reflection or dynamic code loading.
pub async fn run(database: &DatabasePool, class: &str) -> anyhow::Result<()> {
    match class {
        "DatabaseSeeder" => database_seeder::run(database).await,
        // ARTISAN_SEEDER_REGISTRY
        _ => bail!(
            "unknown seeder class {class}; generate/register it in src/seeders/mod.rs"
        ),
    }
}
