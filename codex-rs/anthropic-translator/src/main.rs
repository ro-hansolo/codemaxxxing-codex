use clap::Parser;
use codex_anthropic_translator::server::AppConfig;
use codex_anthropic_translator::server::serve;
use std::net::SocketAddr;
use tokio::net::TcpListener;

/// CLI configuration for the anthropic-translator binary.
#[derive(Debug, Parser)]
#[command(
    name = "codex-anthropic-translator",
    about = "Translate OpenAI Responses API requests to Anthropic Messages",
    version
)]
struct Args {
    /// Address to bind the translator on.
    #[arg(long, env = "TRANSLATOR_LISTEN", default_value = "127.0.0.1:7070")]
    listen: SocketAddr,
    /// Base URL of anthroproxy (the translator appends `/v1/messages`).
    #[arg(
        long,
        env = "TRANSLATOR_UPSTREAM",
        default_value = "http://127.0.0.1:6969"
    )]
    upstream: String,
    /// Anthropic beta-feature identifier to enable. May be passed
    /// multiple times or as a comma-separated list. Each value is
    /// added to the comma-joined `anthropic-beta` header on every
    /// upstream request.
    ///
    /// Common values: `context-management-2025-06-27` (context
    /// editing), `interleaved-thinking-2025-05-14`, compaction
    /// strings, etc. Check the Anthropic docs for the current beta
    /// header strings.
    #[arg(long = "beta", env = "TRANSLATOR_BETA", value_delimiter = ',')]
    beta: Vec<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let listener = TcpListener::bind(args.listen).await?;
    let bound = listener.local_addr()?;
    eprintln!("codex-anthropic-translator listening on http://{bound}");
    eprintln!("forwarding to {}/v1/messages", args.upstream);
    if !args.beta.is_empty() {
        eprintln!("anthropic-beta header: {}", args.beta.join(","));
    }

    serve(
        listener,
        AppConfig {
            upstream_url: args.upstream,
            beta_features: args.beta,
        },
    )
    .await
}
