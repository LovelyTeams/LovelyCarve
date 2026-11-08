extern crate qcow2;

trait OrDie<T> {
    fn or_die(self, msg: &str, path: &str) -> T;
}
impl<T, E: std::fmt::Display> OrDie<T> for Result<T, E> {
    fn or_die(self, msg: &str, path: &str) -> T {
        match self {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{} `{}': {}", msg, path, e);
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    args.remove(0);
    if args.is_empty() {
        println!("Usage: qcow2-dump QCOW2 [...]");
        return;
    }

    for a in args.iter() {
        let f = std::fs::File::open(a).or_die("Error opening file", a);
        let q = qcow2::Qcow2::open(f).or_die("Error reading qcow2", a);
        println!("{:#?}", q);
    }
}
