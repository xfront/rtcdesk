use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;
use stunclient::StunClient;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 读取STUN服务器列表
    let mut f = std::fs::File::open("stunlist.txt")?;
    let mut buf = String::new();
    f.read_to_string(&mut buf)?;

    // 创建通道用于结果收集
    let (tx, mut rx) = mpsc::channel(32);

    // 为每个STUN服务器创建异步任务
    for server in buf.lines() {
        let tx = tx.clone();
        let server = server.to_string();

        tokio::spawn(async move {
            match get_public_address(&server).await {
                Ok((ip, elapsed)) => {
                    // 通过通道发送结果
                    let _ = tx.send(Ok((server, ip, elapsed))).await;
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                }
            }
        });
    }

    // 收集结果
    let mut received = 0;
    let mut resolved_list = Vec::new();
    let timeout = time::sleep(Duration::from_secs(40));
    tokio::pin!(timeout);

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Some(Ok((host, ip, elapsed))) => {
                        received += 1;
                        resolved_list.push((host, ip, elapsed));
                    },
                    Some(Err(e)) => {
                        received += 1;
                        eprintln!("{:?}", e.to_string());
                    },
                    None => break,
                }
            },
            _ = &mut timeout => {
                eprintln!("STUN查询超时");
                break;
            }
        }
    }

    // 按耗时从小到大排序
    resolved_list.sort_by(|a, b| a.2.as_secs_f64().total_cmp(&b.2.as_secs_f64()));

    // 一次性打印所有成功结果
    println!("\n===== 成功解析 {} 个 =====", resolved_list.len());
    for (host, ip, elapsed) in &resolved_list {
        println!("  {} -> {}  ({:.2}s)", host, ip, elapsed.as_secs_f64());
    }

    // 打印统计信息
    println!("\n查询{}个服务器，成功{}个", received, resolved_list.len(),);
    Ok(())
}

async fn get_public_address(stun_server: &str) -> anyhow::Result<(SocketAddr, Duration)> {
    let query_start = std::time::Instant::now();

    // 解析本地地址
    let local_addr: SocketAddr = "0.0.0.0:0".parse()?;

    // 创建UDP socket
    let socket = UdpSocket::bind(&local_addr).await?;

    // 打印查询信息
    let local = socket.local_addr()?;
    println!("查询 {} <- {}", stun_server, local);

    // 解析STUN服务器地址
    let stun_addr = stun_server
        .to_socket_addrs()
        .map_err(|e| anyhow::anyhow!("STUN查询失败 [{}]: {}", stun_server, e))?
        .find(|x| x.is_ipv4())
        .ok_or(anyhow::anyhow!(
            "STUN查询失败[{}],无法解析地址",
            stun_server
        ))?;

    // 创建STUN客户端
    let stun_client = StunClient::new(stun_addr);

    // 异步查询外部地址
    match stun_client.query_external_address_async(&socket).await {
        Ok(external_address) => Ok((external_address, query_start.elapsed())),
        Err(e) => Err(anyhow::anyhow!("STUN查询失败[{}], {}", stun_server, e)),
    }
}
