fn main() {
    // `sqlx::migrate!` embeds the migrations directory; rebuild when it changes.
    println!("cargo:rerun-if-changed=migrations");
}
