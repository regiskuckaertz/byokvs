use anyhow::{anyhow, Context, Result};
use clap::Parser;
use hdrhistogram::Histogram;
use reqwest::blocking::Client;
use std::fs::File;
use std::io::{self, BufRead};
use std::time::{Duration, Instant};

struct Retry {
    backoff: Duration,
    max_backoff: Duration,
}

impl Retry {
    fn new() -> Self {
        Self {
            backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
        }
    }

    fn wait(&mut self) {
        eprintln!("  (waiting {:?} before retry)", self.backoff);
        std::thread::sleep(self.backoff);
        self.backoff = std::cmp::min(self.backoff * 2, self.max_backoff);
    }
}

/// Simple tool to measure latency of a sequence of HTTP queries.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the file containing the queries
    file: String,

    /// Port on which the server is listening (e.g. 8080)
    port: u16,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Build a blocking client – reuse the same connection pool for all requests.
    let client = Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .context("building HTTP client")?;

    // Prepare histogram (1µs to 10 s range, 3 significant figures)
    let mut hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
        .map_err(|e| anyhow!("creating histogram: {}", e))?;

    // Counters
    let mut total_requests = 0usize;
    let start_total = Instant::now();

    // Open and iterate over the query file
    let file =
        File::open(&args.file).with_context(|| format!("opening query file '{}'", args.file))?;
    let reader = io::BufReader::new(file);

    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Split the line – the format is simple enough for `split_whitespace`.
        let mut parts = line.split_whitespace();
        let method = parts
            .next()
            .ok_or_else(|| anyhow!("line {}: missing HTTP method", lineno + 1))?;
        let key = parts
            .next()
            .ok_or_else(|| anyhow!("line {}: missing key", lineno + 1))?;

        // Build the URL for this request.
        let url = format!("http://localhost:{}/{}", args.port, key);

        // Dispatch based on the method.
        match method {
            "PUT" => {
                let value = parts
                    .next()
                    .ok_or_else(|| anyhow!("line {}: missing value for PUT", lineno + 1))?;

                let mut retry = Retry::new();
                loop {
                    let now = Instant::now();
                    match client.put(&url).body(value.to_string()).send() {
                        Ok(resp) => {
                            let elapsed = now.elapsed();
                            if !resp.status().is_success() {
                                eprintln!(
                                    "line {}: PUT {} failed with status {}",
                                    lineno + 1,
                                    url,
                                    resp.status()
                                );
                                retry.wait();
                                continue;
                            }
                            hist.record(elapsed.as_micros() as u64)
                                .map_err(|e| anyhow!("recording latency: {}", e))?;
                            break;
                        }
                        Err(e) => {
                            eprintln!("line {}: PUT {} failed ({})", lineno + 1, url, e);
                            retry.wait();
                        }
                    }
                }
            }
            "GET" => {
                let expected = parts.next().ok_or_else(|| {
                    anyhow!("line {}: missing expected value for GET", lineno + 1)
                })?;

                let mut retry = Retry::new();
                loop {
                    let now = Instant::now();
                    match client.get(&url).send() {
                        Ok(resp) => {
                            let elapsed = now.elapsed();
                            hist.record(elapsed.as_micros() as u64)
                                .map_err(|e| anyhow!("recording latency: {}", e))?;

                            match (resp.status().as_u16(), expected) {
                                (200, "NOT_FOUND") => {
                                    return Err(anyhow!(
                                        "GET {} expected NOT_FOUND but got 200 (line {})",
                                        url,
                                        lineno + 1
                                    ));
                                }
                                (404, "NOT_FOUND") => {
                                    break;
                                }
                                (200, val) => {
                                    let body = resp
                                        .text()
                                        .with_context(|| format!("reading body from {}", url))?;
                                    if body != val {
                                        return Err(anyhow!(
                                            "GET {} expected body '{}', got '{}'",
                                            url,
                                            val,
                                            body
                                        ));
                                    }
                                    break;
                                }
                                (code, _) => {
                                    if code == 404 {
                                        return Err(anyhow!(
                                            "line {}: GET {} expected '{}' but got 404",
                                            lineno + 1,
                                            url,
                                            expected
                                        ));
                                    }
                                    eprintln!(
                                        "line {}: GET {} returned unexpected status {}",
                                        lineno + 1,
                                        url,
                                        code
                                    );
                                    retry.wait();
                                    continue;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("line {}: GET {} failed ({})", lineno + 1, url, e);
                            retry.wait();
                        }
                    }
                }
            }
            other => {
                return Err(anyhow!(
                    "line {}: unsupported method '{}'",
                    lineno + 1,
                    other
                ));
            }
        }

        total_requests += 1;
    }

    let total_elapsed = start_total.elapsed();

    println!("=== Summary ===");
    println!("Total queries processed : {}", total_requests);
    println!(
        "Total wall‑clock time   : {:.3}s",
        total_elapsed.as_secs_f64()
    );

    println!("\nLatency histogram (μs):");
    println!("   min   |   max   |  p50   |  p90   |  p99   |  p99.9");
    println!(
        "{:7} | {:7} | {:7} | {:7} | {:7} | {:7}",
        hist.min(),
        hist.max(),
        hist.value_at_quantile(0.50),
        hist.value_at_quantile(0.90),
        hist.value_at_quantile(0.99),
        hist.value_at_quantile(0.999)
    );

    Ok(())
}
