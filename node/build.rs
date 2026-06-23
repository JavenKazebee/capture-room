fn main() {
    let dist = std::path::Path::new("../ui/dist");
    if !dist.exists() {
        std::fs::create_dir_all(dist).expect("create ../ui/dist placeholder");
    }
}
