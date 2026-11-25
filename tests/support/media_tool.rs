// Native subprocess fixture: configuration belongs to this executable, never global env.
use std::{env, fs, path::PathBuf, thread, time::Duration};
fn main() {
    let executable = env::current_exe().unwrap();
    let scenario = fs::read_to_string(executable.with_extension("scenario")).unwrap();
    let mut lines = scenario.lines();
    let mode = lines.next().unwrap();
    let values: Vec<_> = lines.filter_map(|line| line.split_once('=')).collect();
    let get = |key: &str| values.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
    let args: Vec<String> = env::args().skip(1).collect();
    fs::write(
        executable.with_extension("args"),
        format!("{}\n", args.join("\n")),
    )
    .unwrap();
    if args.iter().any(|a| a == "--dump-single-json") {
        if let Some(json) = get("json") {
            println!("{json}");
            return;
        }
    }
    match mode {
        "ok" => println!("fixture 1"),
        "fail" => std::process::exit(7),
        "probe" | "metadata" | "print" => println!("{}", get("output").unwrap()),
        "delayed-write" => {
            fs::write(get("started").unwrap(), b"started").unwrap();
            thread::sleep(Duration::from_millis(
                get("ms").unwrap_or("2000").parse().unwrap(),
            ));
            fs::write(get("finished").unwrap(), b"finished").unwrap();
        }
        "download-copy" | "conversion-copy" => {
            let out = if mode == "download-copy" {
                PathBuf::from(
                    args[args.iter().position(|a| a == "-o").unwrap() + 1]
                        .replace("%(ext)s", "webm"),
                )
            } else {
                PathBuf::from(args.last().unwrap())
            };
            fs::copy(get("source").unwrap(), &out).unwrap();
            if mode == "download-copy" {
                println!("{}", out.display());
            }
        }
        _ => panic!("unknown fixture scenario: {mode}"),
    }
}
