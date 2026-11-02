fn main() {
    // sqlx::migrate! embeds known files; the directory watch also catches new ones.
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=rollbacks");
}
