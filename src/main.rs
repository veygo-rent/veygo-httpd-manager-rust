use std::{
    net::{TcpListener},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
    sync::{Arc, Mutex},
};
use rand::Rng;
use tokio::{time};

const REPO_URL: &str = "https://github.com/veygo-rent/veygo-httpd-rust.git";
const CLONE_DIR: &str = "veygo-httpd-rust";
const FORWARD_PORT: u16 = 8000;

fn get_random_port() -> Option<u16> {
    let mut rng = rand::rng();
    for _ in 0..10 {
        let port = rng.random_range(8001..9000);
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

fn get_commit_id() -> Option<String> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(CLONE_DIR)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn build_project() -> bool {
    Command::new("cargo")
        .arg("build")
        .arg("--release")
        .current_dir(CLONE_DIR)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn start_server(port: u16) -> Option<Child> {
    Command::new("./target/release/veygo-httpd-rust".to_string())
        .arg(port.to_string())
        .current_dir(CLONE_DIR)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

fn setup_port_forward(from_port: u16, to_port: u16) {
    let _ = Command::new("iptables")
        .args(&["-t", "nat", "-A", "PREROUTING", "-p", "tcp",
                "--dport", &from_port.to_string(), "-j", "REDIRECT", "--to-port", &to_port.to_string()])
        .status();
}

fn clone_or_pull_repo() {
    if Path::new(CLONE_DIR).exists() {
        let _ = Command::new("git")
            .arg("pull")
            .current_dir(CLONE_DIR)
            .status();
    } else {
        let _ = Command::new("git")
            .arg("clone")
            .arg(REPO_URL)
            .arg(CLONE_DIR)
            .status();
    }
}

#[tokio::main]
async fn main() {
    clone_or_pull_repo();
    let mut current_commit = get_commit_id().unwrap_or_default();
    let mut child = None;

    let port = get_random_port().expect("No available ports");
    if build_project() {
        if let Some(new_child) = start_server(port) {
            setup_port_forward(FORWARD_PORT, port);
            child = Some(new_child);
            println!("Server running on port {}, forwarded to {}", port, FORWARD_PORT);
        }
    }

    let child_arc = Arc::new(Mutex::new(child));

    let monitor_handle = {
        let child_arc = Arc::clone(&child_arc);
        tokio::spawn(async move {
            loop {
                time::sleep(Duration::from_secs(10)).await;
                clone_or_pull_repo();
                if let Some(new_commit) = get_commit_id() {
                    if new_commit != current_commit {
                        println!("New commit found. Rebuilding...");
                        current_commit = new_commit;
                        if build_project() {
                            if let Some(new_port) = get_random_port() {
                                if let Some(new_child) = start_server(new_port) {
                                    setup_port_forward(FORWARD_PORT, new_port);
                                    if let Some(mut old_child) = child_arc.lock().unwrap().take() {
                                        let _ = old_child.kill();
                                        println!("Old server killed.");
                                    }
                                    *child_arc.lock().unwrap() = Some(new_child);
                                    println!("New server running on port {}", new_port);
                                }
                            }
                        }
                    }
                }
            }
        })
    };

    monitor_handle.await.unwrap();
}
