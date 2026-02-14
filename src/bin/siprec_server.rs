//! SIPREC Recording Server CLI.

use clap::Parser;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use siprec_server::server::{ServerConfig, SiprecServer};
use siprec_server::storage::{
    HttpMethod, HttpStorage, HttpStorageConfig, LocalStorage, MultiStorage, S3Storage,
    S3StorageConfig, StorageBackend,
};

#[derive(Parser)]
#[command(
    name = "siprec-server",
    about = "SIPREC (RFC 7865/7866) recording server by 5060 Solutions"
)]
struct Args {
    /// SIP listen port
    #[arg(long, default_value_t = 5060)]
    sip_port: u16,

    /// Local IP for SDP answers
    #[arg(long)]
    local_ip: String,

    /// RTP port range start
    #[arg(long, default_value_t = 20000)]
    rtp_port_min: u16,

    /// RTP port range end
    #[arg(long, default_value_t = 30000)]
    rtp_port_max: u16,

    /// Recording output directory
    #[arg(long, default_value = "./recordings")]
    recording_dir: String,

    /// S3 bucket for uploading recordings (enables S3 backend)
    #[arg(long)]
    s3_bucket: Option<String>,

    /// S3 key prefix for recordings
    #[arg(long, default_value = "recordings/")]
    s3_prefix: String,

    /// Delete local files after S3 upload
    #[arg(long, default_value_t = false)]
    s3_delete_local: bool,

    /// HTTP webhook URL for uploading recordings (enables HTTP backend)
    #[arg(long)]
    http_url: Option<String>,

    /// HTTP method for webhook (post or put)
    #[arg(long, default_value = "post")]
    http_method: String,

    /// HTTP Authorization header value
    #[arg(long)]
    http_auth: Option<String>,

    /// Delete local files after HTTP upload
    #[arg(long, default_value_t = false)]
    http_delete_local: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(true)
        .init();

    let args = Args::parse();

    let config = ServerConfig {
        sip_port: args.sip_port,
        rtp_port_min: args.rtp_port_min,
        rtp_port_max: args.rtp_port_max,
        recording_dir: args.recording_dir,
        local_ip: args.local_ip,
    };

    // Build storage backend(s)
    let mut backends: Vec<Box<dyn StorageBackend>> = Vec::new();

    // Always include local storage as the first backend
    backends.push(Box::new(LocalStorage));

    // Add S3 if configured
    if let Some(bucket) = args.s3_bucket {
        let s3_config = S3StorageConfig {
            bucket,
            key_prefix: args.s3_prefix,
            delete_local: args.s3_delete_local,
        };
        let s3 = S3Storage::from_defaults(s3_config).await;
        backends.push(Box::new(s3));
    }

    // Add HTTP webhook if configured
    if let Some(url) = args.http_url {
        let method = match args.http_method.to_lowercase().as_str() {
            "put" => HttpMethod::Put,
            _ => HttpMethod::Post,
        };
        let http_config = HttpStorageConfig {
            url,
            method,
            auth_header: args.http_auth,
            delete_local: args.http_delete_local,
        };
        backends.push(Box::new(HttpStorage::new(http_config)));
    }

    // Use multi-backend if more than one, otherwise just the single backend
    let storage: Arc<dyn StorageBackend> = if backends.len() == 1 {
        Arc::from(backends.into_iter().next().unwrap())
    } else {
        Arc::new(MultiStorage::new(backends))
    };

    let server = SiprecServer::new(config, storage);
    server.run().await?;

    Ok(())
}
