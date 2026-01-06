pub fn run() {
    let key = format!("sk-{}", uuid::Uuid::new_v4().simple());
    println!("{}", key);
}
