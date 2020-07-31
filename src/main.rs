use ansible_rs::ParallelSshProps;
use ansible_rs::Response;
use clap::{App, Arg};
use color_backtrace;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::path::Path;
use std::process::exit;
use std::thread::spawn;

use async_executor::LocalExecutor;
use futures_channel::mpsc::{channel, Receiver, Sender};
use std::time::UNIX_EPOCH;

fn hosts_builder(path: &Path) -> Vec<String> {
    let file = File::open(path).expect("Unable to open the file");
    let reader = BufReader::new(file);
    reader
        .lines()
        .map(|l| l.unwrap() + ":22")
        .collect::<Vec<String>>()
}

#[derive(Deserialize, Debug, Clone)]
struct OutputProps {
    save_to_file: bool,
    filename: Option<String>,
    pretty_format: bool,
    show_progress: bool,
    keep_incremental_data: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
struct Config {
    threads: usize,
    output: OutputProps,
    command: String,
    timeout: u32,
}

impl Default for OutputProps {
    fn default() -> Self {
        OutputProps {
            save_to_file: false,
            filename: None,
            pretty_format: false,
            show_progress: false,
            keep_incremental_data: Some(false),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            threads: 10,
            command: String::default(),
            output: OutputProps::default(),
            timeout: 60,
        }
    }
}

fn get_config(path: &Path) -> Config {
    let f = match fs::read_to_string(path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Failed reading config. Using default values : {}", e);
            return Config::default();
        }
    };
    match toml::from_str(f.as_str()) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Error parsing config:{}", e);
            Config::default()
        }
    }
}

fn save_to_file(conf: &Config, data: Vec<Response>) {
    let filename = match &conf.output.filename {
        None => {
            eprintln!("Filename to save is not given. Printing to stdout.");
            save_to_console(&conf, &data);
            return;
        }
        Some(a) => Path::new(a.as_str()),
    };

    let file = match File::create(filename) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error saving content to file:{}", e);
            save_to_console(&conf, &data);
            return;
        }
    };
    if conf.output.pretty_format {
        match serde_json::to_writer_pretty(file, &data) {
            Ok(_) => println!("Saved successfully"),
            Err(e) => eprintln!("Error saving: {}", e),
        };
    } else {
        match serde_json::to_writer(file, &data) {
            Ok(_) => println!("Saved successfully"),
            Err(e) => eprintln!("Error saving: {}", e),
        }
    }
}

fn save_to_console(conf: &Config, data: &Vec<Response>) {
    if conf.output.pretty_format {
        println!("{}", serde_json::to_string_pretty(&data).unwrap())
    } else {
        println!("{}", serde_json::to_string(&data).unwrap())
    }
}

fn main() {
    color_backtrace::install();
    let args = App::new("SSH analyzer")
        .arg(
            Arg::with_name("config")
                .short("c")
                .long("config")
                .help("Path to config file")
                .required(false)
                .takes_value(true)
                .default_value("config.toml"),
        )
        .arg(
            Arg::with_name("hosts")
                .long("hosts")
                .help("Path to file with hosts")
                .required(true)
                .takes_value(true),
        )
        .get_matches();
    let hosts = hosts_builder(Path::new(&args.value_of("hosts").unwrap()));
    let config = get_config(Path::new(&args.value_of("config").unwrap()));
    dbg!(&config);
    let props = config.output.clone();
    let queue_len = hosts.len() as u64;
    let datetime = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        .to_string();
    let incremental_name = format!("incremental_{}.json", &datetime);
    let inc_for_closure = incremental_name.clone();
    let (rx, processor) = ParallelSshProps::new(10);
    spawn(move || incremental_save(rx, &props, queue_len, incremental_name.as_str()));
    let local_ex = LocalExecutor::new();
    local_ex.run(async {
        processor
            .parallel_ssh_process(hosts, &config.command.clone())
            .await;
    });

    match config.output.keep_incremental_data {
        Some(true) => {}
        Some(false) => {
            match std::fs::remove_file(Path::new(&inc_for_closure)) {
                Ok(_) => (),
                Err(e) => eprintln!("Error removing temp file : {}", e),
            };
        }
        None => {
            match std::fs::remove_file(Path::new(&inc_for_closure)) {
                Ok(_) => (),
                Err(e) => eprintln!("Error removing temp file : {}", e),
            };
        }
    };
}

fn progress_bar_creator(queue_len: u64) -> ProgressBar {
    let total_hosts_processed = ProgressBar::new(queue_len);
    let total_style = ProgressStyle::default_bar()
        .template("{eta} {wide_bar} Hosts processed: {pos}/{len} Speed: {per_sec} {msg}")
        .progress_chars("##-");
    total_hosts_processed.set_style(total_style);

    total_hosts_processed
}

fn incremental_save(
    mut rx: Receiver<Response>,
    _props: &OutputProps,
    queue_len: u64,
    filename: &str,
) {
    let mut file = match File::create(Path::new(filename)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("incremental salving failed. : {}", e);
            return;
        }
    };
    //    let total = progress_bar_creator(queue_len);
    let total = ProgressBar::hidden();
    let mut ok = 0;
    let mut ko = 0;
    file.write_all(b"[\r\n")
        .expect("Writing for incremental saving failed");
    for _ in 0..=queue_len {
        let received = match rx.try_next() {
            Ok(a) => match a {
                Some(a) => a,
                None => continue,
            },
            Err(e) => {
                eprintln!("Error receiving stats :{}", e);
                exit(1);
            }
        };
        if received.status {
            ok += 1
        } else {
            ko += 1
        };
        total.inc(1);
        total.set_message(&format!("OK: {}, Failed: {}", ok, ko));
        let mut data = serde_json::to_string_pretty(&received).unwrap();
        data += ",\n";
        file.write_all(data.as_bytes())
            .expect("Writing for incremental saving failed");
    }
    file.write_all(b"\n]")
        .expect("Writing for incremental saving failed");
}
