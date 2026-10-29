fn main() {
    println!(
        "{}",
        reel_api::openapi()
            .to_pretty_json()
            .expect("serialize openapi")
    );
}
