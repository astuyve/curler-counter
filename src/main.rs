use aws_config::BehaviorVersion;
use aws_sdk_cloudwatch::types::MetricDatum;
use aws_sdk_cloudwatch;
use reqwest;
use serde_json;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use std::env;
use tokio::task::JoinSet;

// {"handlerRunTime":1715529543389,
// "staticInitTime":1715529543384,
// "coldStartResult":true,
// "processUptime":0.116852724}%
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FunctionResponse {
    cold_start_result: bool,
    process_uptime: f64,
    handler_run_time: i64,
    static_init_time: i64,
    #[serde(skip)]
    request_duration: Duration,
    #[serde(skip)]
    function_name: String,
}

fn get_function_urls(mut map: HashMap<&'static str, String>) -> HashMap<&str, String> {
    if let Ok(url) = env::var("LOCAL_COLD_START_LAMBDA") {
        map.insert("aws", url);
    }
    if let Ok(url) = env::var("LOCAL_COLD_START_VERCEL") {
        map.insert("vercel", url);
    }
    if let Ok(url) = env::var("LOCAL_COLD_START_LWA") {
        map.insert("lwa", url);
    }
    if let Ok(url) = env::var("LOCAL_COLD_START_HONO") {
        map.insert("hono", url);
    }
    if let Ok(url) = env::var("LOCAL_COLD_START_SERVERLESS_HTTP") {
        map.insert("serverless_http", url);
    }
    map
}

#[tokio::main]
async fn main() {
    let function_urls = get_function_urls(HashMap::new());
    loop {
        let mut set = JoinSet::new();
        let mut results_with_duration = Vec::new();
        for (function_name, url) in function_urls.iter() {
            let mut i = 0;
            while i < 3 {
                let moved_name = function_name.to_string();
                let moved_url = url.clone();
                set.spawn(async move {
                    let time_start = std::time::Instant::now();
                    let response = reqwest::get(moved_url).await.expect("no response").text().await.unwrap();
                    let request_duration = time_start.elapsed();
                    let mut json_result = serde_json::from_str::<FunctionResponse>(&response).unwrap();
                    json_result.request_duration = request_duration;
                    json_result.function_name = moved_name.to_string();
                    json_result
                });
                i += 1;
            }
            
            while let Some(res) = set.join_next().await {
                let json_result = res.unwrap();
                if json_result.cold_start_result && json_result.process_uptime <= 1.0 {
                    results_with_duration.push(json_result);
                } else {
                    println!("Skipping: {}, no cold start detected", function_name);
                }
            }
        }
        if results_with_duration.is_empty() {
            println!("No cold start detected, waiting 15 minutes");
        } else {
            println!("Sending metrics");
            put_metrics(results_with_duration).await;
        }
        tokio::time::sleep(Duration::from_secs(60 * 15)).await;
    }
}

async fn put_metrics(json_results: Vec<FunctionResponse>) {
    let region_provider = aws_config::Region::new("us-east-1");

    let metrics = json_results
        .iter()
        .map(|json_result| {
            MetricDatum::builder()
                .metric_name(json_result.function_name.clone())
                .timestamp(std::time::SystemTime::now().into())
                .value(json_result.request_duration.as_millis() as f64)
                .unit("Milliseconds".into())
                .build()
        })
        .collect::<Vec<MetricDatum>>();
    println!("METRICS PAYLOAD: {:?}", metrics);
    let config = aws_config::defaults(BehaviorVersion::latest())
        .region(region_provider)
        .load()
        .await;
    let cloudwatch = aws_sdk_cloudwatch::Client::new(&config);
    cloudwatch
        .put_metric_data()
        .namespace("aj-local-metrics")
        .set_metric_data(Some(metrics))
        .send()
        .await
        .expect("failed to put metric data");
}
