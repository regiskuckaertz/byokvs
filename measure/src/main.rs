use anyhow::{anyhow, Context, Result};
use clap::Parser;
use hdrhistogram::Histogram;
use reqwest::blocking::Client;
use std::fs::File;
use std::io::{self, BufRead};
use std::time::Instant;

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
        .danger_accept_invalid_certs(true) // not needed for http, but keeps the builder happy
        .build()
        .context("building HTTP client")?;

    // Prepare histogram (1µs to 10 s range, 3 significant figures)
    let mut hist = Histogram::<u64>::new_with_bounds(1, 10_000_000, 3)
        .map_err(|e| anyhow!("creating histogram: {}", e))?;

    // Counters
    let mut total_requests = 0usize;
    let start_total = Instant::now();

    // Open and iterate over the query file
    let file = File::open(&args.file)
        .with_context(|| format!("opening query file '{}'", args.file))?;
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
                // Measure latency
                let now = Instant::now();
                let resp = client
                    .put(&url)
                    .body(value.to_string())
                    .send()
                    .with_context(|| format!("PUT request to {}", url))?;
                let elapsed = now.elapsed();
                hist.record(elapsed.as_micros() as u64)
                    .map_err(|e| anyhow!("recording latency: {}", e))?;

                // Expect 200 (or 201) – any non‑2xx is an error.
                if !resp.status().is_success() {
                    return Err(anyhow!(
                        "PUT {} failed with status {} (line {})",
                        url,
                        resp.status(),
                        lineno + 1
                    ));
                }
            }
            "GET" => {
                let expected = parts
                    .next()
                    .ok_or_else(|| anyhow!("line {}: missing expected value for GET", lineno + 1))?;

                // Measure latency
                let now = Instant::now();
                let resp = client
                    .get(&url)
                    .send()
                    .with_context(|| format!("GET request to {}", url))?;
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
                        // Expected, nothing else to verify.
                    }
                    (200, val) => {
                        // Verify body matches expected value.
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
                    }
                    (code, _) => {
                        return Err(anyhow!(
                            "GET {} returned unexpected status {} (line {})",
                            url,
                            code,
                            lineno + 1
                        ));
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

    // ----- Output -----
    println!("=== Summary ===");
    println!("Total queries processed : {}", total_requests);
    println!(
        "Total wall‑clock time   : {:.3}s",
        total_elapsed.as_secs_f64()
    );

    // Human‑friendly histogram (microseconds)
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
