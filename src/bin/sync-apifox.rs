#[tokio::main]
async fn main() {
    if let Err(error) = workspace_proto_gen::apifox::run(std::env::args().skip(1).collect()).await {
        eprintln!("错误: {error:#}");
        std::process::exit(1);
    }
}
