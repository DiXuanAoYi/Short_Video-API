//! 网络：按网站分流（直连 / 系统代理 / 自定义代理）、客户端缓存、全局限速、同一网站的请求间隔。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use url::Url;

use crate::error::{AppError, AppResult, ErrorKind};
use crate::providers::MOBILE_UA;
use crate::settings::{NetworkSettings, Route};

#[derive(Clone)]
pub struct Clients {
    /// 解析用：有总超时
    pub api: reqwest::Client,
    /// 下载用：只有读超时，适合大文件
    pub download: reqwest::Client,
}

#[derive(Default)]
pub struct NetManager {
    cache: Mutex<HashMap<String, Clients>>,
    site_last: Mutex<HashMap<String, Instant>>,
    limiter: SpeedLimiter,
}

impl NetManager {
    /// 设置变化后清空客户端缓存。
    pub fn invalidate(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn route_for(&self, net: &NetworkSettings, url: &str) -> Route {
        Url::parse(url).ok().and_then(|u| u.host_str().map(|h| net.route_for_host(h))).unwrap_or_else(|| net.default_route.clone())
    }

    /// 访问某个地址应使用的客户端。
    pub fn clients_for(&self, net: &NetworkSettings, url: &str) -> AppResult<Clients> {
        let route = self.route_for(net, url);
        self.clients_for_route(net, &route)
    }

    pub fn clients_for_route(&self, net: &NetworkSettings, route: &Route) -> AppResult<Clients> {
        let proxy_url = match route {
            Route::Proxy(id) => {
                Some(net.proxy_url(id).ok_or_else(|| AppError::invalid(format!("代理规则引用了不存在的代理“{id}”，请检查网络设置。")))?.to_string())
            }
            _ => None,
        };
        let key = format!("{route:?}|{}", proxy_url.as_deref().unwrap_or(""));
        if let Some(c) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return Ok(c.clone());
        }
        let clients = build_clients(route, proxy_url.as_deref())?;
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, clients.clone());
        Ok(clients)
    }

    /// 同一网站两次解析请求之间至少间隔 `interval`，降低被风控的概率。
    pub async fn wait_turn(&self, site: &str, interval: Duration) {
        if interval.is_zero() {
            return;
        }
        let wait = {
            let mut map = self.site_last.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let next = map.get(site).map(|t| *t + interval).unwrap_or(now);
            let slot = next.max(now);
            map.insert(site.to_string(), slot);
            slot.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    pub async fn throttle(&self, bytes: usize, limit_kbps: u64) {
        self.limiter.consume(bytes as u64, limit_kbps).await;
    }

    /// 测试某个地址在当前规则下能否访问，返回耗时（毫秒）。
    pub async fn test(&self, net: &NetworkSettings, url: &str) -> AppResult<RouteTest> {
        let route = self.route_for(net, url);
        let clients = self.clients_for_route(net, &route)?;
        let t0 = Instant::now();
        let resp = clients.api.get(url).timeout(Duration::from_secs(10)).send().await.map_err(|e| {
            let msg = if e.is_timeout() {
                "连接超时".to_string()
            } else if e.is_connect() {
                "无法连接（代理不可用或网站不可达）".to_string()
            } else {
                e.without_url().to_string()
            };
            AppError::new(ErrorKind::Network, msg)
        })?;
        Ok(RouteTest { route, status: resp.status().as_u16(), millis: t0.elapsed().as_millis() as u64 })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteTest {
    pub route: Route,
    pub status: u16,
    pub millis: u64,
}

fn build_clients(route: &Route, proxy_url: Option<&str>) -> AppResult<Clients> {
    let apply = |b: reqwest::ClientBuilder| -> AppResult<reqwest::ClientBuilder> {
        Ok(match route {
            Route::Direct => b.no_proxy(),
            // 默认构造会读取系统代理设置和 HTTP(S)_PROXY 环境变量
            Route::System => b,
            Route::Proxy(_) => {
                let url = proxy_url.unwrap_or_default();
                b.proxy(reqwest::Proxy::all(url).map_err(|e| AppError::invalid(format!("代理地址格式不正确：{e}")))?)
            }
        })
    };
    let api = apply(
        reqwest::Client::builder()
            .user_agent(MOBILE_UA)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true),
    )?
    .build()
    .map_err(|e| AppError::msg(format!("创建网络客户端失败：{e}")))?;
    let download = apply(
        reqwest::Client::builder()
            .user_agent(MOBILE_UA)
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10)),
    )?
    .build()
    .map_err(|e| AppError::msg(format!("创建网络客户端失败：{e}")))?;
    Ok(Clients { api, download })
}

/// 全局限速：以 100 毫秒为窗口分配字节额度，所有下载任务共享。
#[derive(Default)]
pub struct SpeedLimiter {
    state: Mutex<(Option<Instant>, u64)>,
}

impl SpeedLimiter {
    pub async fn consume(&self, bytes: u64, limit_kbps: u64) {
        if limit_kbps == 0 {
            return;
        }
        let per_window = (limit_kbps * 1024 / 10).max(1);
        loop {
            let wait = {
                let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let now = Instant::now();
                let start = *st.0.get_or_insert(now);
                if now.duration_since(start) >= Duration::from_millis(100) {
                    *st = (Some(now), 0);
                }
                if st.1 < per_window {
                    st.1 += bytes;
                    None
                } else {
                    Some(Duration::from_millis(100).saturating_sub(now.duration_since(st.0.unwrap_or(now))))
                }
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d.max(Duration::from_millis(5))).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ProxyDef, RouteRule};

    #[test]
    fn missing_proxy_is_a_clear_error() {
        let net = NetworkSettings { default_route: Route::Proxy("nope".into()), rules: vec![], proxies: vec![] };
        let err = NetManager::default().clients_for(&net, "https://x.com/").err().unwrap();
        assert!(err.to_string().contains("不存在的代理"));
    }

    #[test]
    fn clients_are_cached_per_route() {
        let net = NetworkSettings {
            default_route: Route::System,
            rules: vec![RouteRule { pattern: "a.com".into(), route: Route::Proxy("p".into()) }],
            proxies: vec![ProxyDef { id: "p".into(), name: "本地".into(), url: "http://127.0.0.1:7890".into() }],
        };
        let m = NetManager::default();
        m.clients_for(&net, "https://a.com/").unwrap();
        m.clients_for(&net, "https://b.a.com/").unwrap();
        m.clients_for(&net, "https://c.com/").unwrap();
        assert_eq!(m.cache.lock().unwrap().len(), 2);
        m.invalidate();
        assert!(m.cache.lock().unwrap().is_empty());
    }

    #[test]
    fn invalid_proxy_url_is_reported() {
        let net = NetworkSettings {
            default_route: Route::Proxy("p".into()),
            rules: vec![],
            proxies: vec![ProxyDef { id: "p".into(), name: "x".into(), url: "::bad::".into() }],
        };
        assert!(NetManager::default().clients_for(&net, "https://x.com/").is_err());
    }

    #[tokio::test]
    async fn wait_turn_spaces_requests() {
        let m = NetManager::default();
        let t0 = Instant::now();
        m.wait_turn("bilibili", Duration::from_millis(120)).await;
        m.wait_turn("bilibili", Duration::from_millis(120)).await;
        m.wait_turn("other", Duration::from_millis(120)).await;
        let e = t0.elapsed();
        assert!(e >= Duration::from_millis(110) && e < Duration::from_millis(400), "{e:?}");
    }

    #[tokio::test]
    async fn speed_limiter_slows_down() {
        let l = SpeedLimiter::default();
        let t0 = Instant::now();
        // 限速 100 KB/s，消耗 30 KB 约需 0.2–0.3 秒
        for _ in 0..30 {
            l.consume(1024, 100).await;
        }
        assert!(t0.elapsed() >= Duration::from_millis(180), "{:?}", t0.elapsed());
        let t1 = Instant::now();
        l.consume(10_000_000, 0).await;
        assert!(t1.elapsed() < Duration::from_millis(10));
    }
}
