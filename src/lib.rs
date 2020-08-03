use anyhow::Error;
use async_ssh2::Session;
use futures::prelude::*;
use futures::stream::FuturesUnordered;
use futures::Future;
use serde::Serialize;
use smol::Async;
use smol::{blocking, reader};
use std::fmt::Display;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

#[macro_use]
extern crate derive_builder;

#[derive(Serialize, Debug, Clone)]
pub struct Response {
    pub result: String,
    pub hostname: String,
    pub process_time: Duration,
    pub status: bool,
}

#[derive(Builder)]
#[builder(setter(into))]
pub struct ParallelSshProps {
    maximum_connections: usize,
    agent_parallelism: usize,
    timeout_socket: Duration,
    timeout_ssh: Duration,
}

async fn process_host<A>(
    hostname: A,
    command: Arc<String>,
    timeout_socket: Duration,
    agent_pool: Arc<Semaphore>,
    threads_limit: Arc<Semaphore>,
) -> Response
where
    A: ToSocketAddrs + Display + Sync + Clone + Send,
{
    let start_time = Instant::now();
    let result = process_host_inner(
        hostname.clone(),
        timeout_socket,
        command,
        agent_pool,
        threads_limit,
    )
    .await;
    let process_time = Instant::now() - start_time;
    let response = match result {
        Ok(a) => Response {
            result: a,
            hostname: hostname.to_string(),
            process_time,
            status: true,
        },
        Err(e) => Response {
            result: e.to_string(),
            hostname: hostname.to_string(),
            process_time,
            status: false,
        },
    };

    response
}

async fn process_host_inner<A>(
    hostname: A,
    timeout_socket: Duration,
    command: Arc<String>,
    agent_pool: Arc<Semaphore>,
    threads_pool: Arc<Semaphore>,
) -> Result<String, Error>
where
    A: ToSocketAddrs + Display + Sync + Clone + Send,
{
    let _threads_guard = threads_pool.acquire().await;
    let address = &hostname
        .to_socket_addrs()?
        .next()
        .ok_or(Error::msg("Failed converting address"))?;

    let sync_stream = TcpStream::connect_timeout(&address, timeout_socket)?;
    let tcp = Async::new(sync_stream)?;
    let mut sess =
        Session::new().map_err(|_e| Error::msg(format!("Error initializing session")))?;
    // dbg!("Session initialized");
    const TIMEOUT: u32 = 6000;
    sess.set_timeout(TIMEOUT);
    sess.set_tcp_stream(tcp)?;
    sess.handshake()
        .await
        .map_err(|e| Error::msg(format!("Failed establishing handshake: {}", e)))?;
    // dbg!("Handshake done");
    let guard = agent_pool.acquire().await;
    let mut agent = sess
        .agent()
        .map_err(|e| Error::msg(format!("Failed connecting to agent: {}", e)))?;
    agent.connect().await?;
    // dbg!("Agent connected");
    sess.userauth_agent("scan")
        .await
        .map_err(|e| Error::msg(format!("Error connecting via agent: {}", e)))?;
    drop(guard); //todo test, that it really works
    let mut channel = sess
        .channel_session()
        .await
        .map_err(|e| Error::msg(format!("Failed opening channel: {}", e)))?;
    // dbg!("Chanel opened");
    channel
        .exec(&command)
        .await
        .map_err(|e| Error::msg(format!("Failed executing command in channel: {}", e)))?;

    // let mut command_stdout = reader(channel.stream(0));
    let mut reader = reader(channel.stream(0));
    let mut channel_buffer = String::with_capacity(4096);
    reader
        .read_to_string(&mut channel_buffer)
        .await
        .map_err(|e| Error::msg(format!("Error reading result of work: {}", e)))?;
    Ok(channel_buffer)
}

impl ParallelSshProps {
    pub fn new() -> Self {
        Self {
            maximum_connections: 1,
            agent_parallelism: 1,
            timeout_socket: Duration::new(1, 0),
            timeout_ssh: Duration::from_secs(600),
        }
    }

    pub async fn parallel_ssh_process<A: 'static>(
        self,
        hosts: Vec<A>,
        command: &str,
    ) -> FuturesUnordered<impl Future<Output = Response>>
    where
        A: Display + ToSocketAddrs + Send + Sync + Clone,
    {
        let num_of_threads = Arc::new(Semaphore::new(self.maximum_connections));
        let futures = FuturesUnordered::new();
        let agent_parallelism = Arc::new(Semaphore::new(self.agent_parallelism));
        let command = Arc::new(command.to_string());

        for host in hosts {
            let command = command.clone();
            let process_result = process_host(
                host,
                command,
                self.timeout_socket,
                agent_parallelism.clone(),
                num_of_threads.clone(),
            );
            futures.push(process_result);
        }
        futures
    }
}
