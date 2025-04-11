use std::{
    net::{TcpListener},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
    sync::{Arc, Mutex},
};
use rand::Rng;
use tokio::{time, task::JoinHandle};

use std::net::IpAddr;
use std::str::FromStr;

const REPO_URL: &str = "https://github.com/veygo-rent/veygo-httpd-rust.git";
const CLONE_DIR: &str = "target/veygo-httpd-rust";
const FORWARD_PORT: u16 = 8000;

fn get_random_port() -> Option<u16> {
    let mut rng = rand::rng();
    for _ in 0..10 {
        let port = rng.random_range(8001..9000);
        let addr = IpAddr::from_str("::0").unwrap();
        if TcpListener::bind((addr, port)).is_ok() {
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
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()
}

async fn setup_port_forward_tokio(from_port: u16, to_port: u16) -> JoinHandle<()> {
    tokio::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(("::0", from_port)).await {
            Ok(listener) => listener,
            Err(e) => {
                eprintln!("Failed to bind to forward port {}: {}", from_port, e);
                return;
            }
        };

        while let Ok((inbound, _)) = listener.accept().await {
            match tokio::net::TcpStream::connect(("::0", to_port)).await {
                Ok(outbound) => {
                    tokio::spawn(async move {
                        let (mut ri, mut wi) = tokio::io::split(inbound);
                        let (mut ro, mut wo) = tokio::io::split(outbound);

                        let client_to_server = tokio::io::copy(&mut ri, &mut wo);
                        let server_to_client = tokio::io::copy(&mut ro, &mut wi);

                        let _ = tokio::join!(client_to_server, server_to_client);
                    });
                }
                Err(e) => eprintln!("Failed to connect to target: {}", e),
            }
        }
    })
}

fn clone_or_pull_repo() {
    if Path::new(CLONE_DIR).exists() {
        let _ = Command::new("git")
            .arg("pull")
            .arg("-q")
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

fn run_migration() {
    println!("Running Diesel migrations...");
    let result = Command::new("diesel")
        .arg("migration")
        .arg("run")
        .current_dir(CLONE_DIR)
        .status();
    match result {
        Ok(status) if status.success() => {
            println!("Diesel migrations ran successfully.");
        }
        Ok(status) => {
            eprintln!("Diesel migrations exited with status code: {:?}", status.code());
        }
        Err(err) => {
            eprintln!("Failed to run Diesel migrations: {}", err);
        }
    }
}

#[tokio::main]
async fn main() {
    clone_or_pull_repo();
    let mut current_commit = get_commit_id().unwrap_or_default();
    let mut child = None;
    let forward_handle_arc = Arc::new(Mutex::new(None::<JoinHandle<()>>));

    let port = get_random_port().expect("No available ports");
    if build_project() {
        run_migration();
        if let Some(new_child) = start_server(port) {
            let forward_handle = setup_port_forward_tokio(FORWARD_PORT, port).await;
            *forward_handle_arc.lock().unwrap() = Some(forward_handle);
            child = Some(new_child);
            println!("Server running on port {}, forwarded to {}", port, FORWARD_PORT);
        }
    }

    let child_arc = Arc::new(Mutex::new(child));

    let monitor_handle = {
        let child_arc = Arc::clone(&child_arc);
        let forward_handle_arc = Arc::clone(&forward_handle_arc);
        tokio::spawn(async move {
            loop {
                time::sleep(Duration::from_secs(60)).await;
                clone_or_pull_repo();
                if let Some(new_commit) = get_commit_id() {
                    if new_commit != current_commit {
                        println!("New commit {} found. Rebuilding...", new_commit);
                        current_commit = new_commit;
                        if build_project() {
                            run_migration();
                            if let Some(new_port) = get_random_port() {
                                if let Some(new_child) = start_server(new_port) {
                                    if let Some(mut old_child) = child_arc.lock().unwrap().take() {
                                        let _ = old_child.kill();
                                        let _ = old_child.wait();
                                        println!("Old server killed.");
                                    }
                                    let old_forward_handle = {
                                        let mut lock = forward_handle_arc.lock().unwrap();
                                        lock.take()
                                    };
                                    if let Some(old_forward_handle) = old_forward_handle {
                                        old_forward_handle.abort();
                                        let _ = old_forward_handle.await;
                                        time::sleep(Duration::from_secs(5)).await;
                                        println!("Old forwarder aborted.");
                                    }
                                    // Introduce a short delay to allow the OS to release the port
                                    let new_forward_handle = setup_port_forward_tokio(FORWARD_PORT, new_port).await;
                                    *forward_handle_arc.lock().unwrap() = Some(new_forward_handle);
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
