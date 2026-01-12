use log::{debug, info};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Счетчик для метрик
#[derive(Debug, Clone)]
pub struct MetricsCounter {
    packets_received: Arc<AtomicU64>,
    packets_sent: Arc<AtomicU64>,
    bytes_received: Arc<AtomicU64>,
    bytes_sent: Arc<AtomicU64>,
    connections_total: Arc<AtomicU64>,
    connections_active: Arc<AtomicU64>,
    start_time: Instant,
}

impl MetricsCounter {
    pub fn new() -> Self {
        Self {
            packets_received: Arc::new(AtomicU64::new(0)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            bytes_received: Arc::new(AtomicU64::new(0)),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            connections_total: Arc::new(AtomicU64::new(0)),
            connections_active: Arc::new(AtomicU64::new(0)),
            start_time: Instant::now(),
        }
    }

    pub fn increment_packets_received(&self, count: u64) {
        self.packets_received.fetch_add(count, Ordering::Relaxed);
    }

    pub fn increment_packets_sent(&self, count: u64) {
        self.packets_sent.fetch_add(count, Ordering::Relaxed);
    }

    pub fn increment_bytes_received(&self, bytes: u64) {
        self.bytes_received.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn increment_bytes_sent(&self, bytes: u64) {
        self.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn increment_connections(&self) {
        self.connections_total.fetch_add(1, Ordering::Relaxed);
        self.connections_active.fetch_add(1, Ordering::Relaxed);
    }

    pub fn decrement_connections(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn get_packets_received(&self) -> u64 {
        self.packets_received.load(Ordering::Relaxed)
    }

    pub fn get_packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }

    pub fn get_bytes_received(&self) -> u64 {
        self.bytes_received.load(Ordering::Relaxed)
    }

    pub fn get_bytes_sent(&self) -> u64 {
        self.bytes_sent.load(Ordering::Relaxed)
    }

    pub fn get_connections_total(&self) -> u64 {
        self.connections_total.load(Ordering::Relaxed)
    }

    pub fn get_connections_active(&self) -> u64 {
        self.connections_active.load(Ordering::Relaxed)
    }

    pub fn get_uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn format_stats(&self) -> String {
        let uptime = self.get_uptime();
        let hours = uptime.as_secs() / 3600;
        let minutes = (uptime.as_secs() % 3600) / 60;
        let seconds = uptime.as_secs() % 60;

        format!(
            "Metrics:\n\
             - Uptime: {:02}:{:02}:{:02}\n\
             - Active Connections: {}\n\
             - Total Connections: {}\n\
             - Packets Received: {}\n\
             - Packets Sent: {}\n\
             - Bytes Received: {} MB\n\
             - Bytes Sent: {} MB",
            hours,
            minutes,
            seconds,
            self.get_connections_active(),
            self.get_connections_total(),
            self.get_packets_received(),
            self.get_packets_sent(),
            self.get_bytes_received() / 1_000_000,
            self.get_bytes_sent() / 1_000_000
        )
    }

    pub fn log_stats(&self) {
        info!("{}", self.format_stats());
    }
}

impl Default for MetricsCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Генератор уникальных ID
pub struct IdGenerator {
    counter: Arc<AtomicU64>,
    prefix: String,
}

impl IdGenerator {
    pub fn new(prefix: &str) -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(0)),
            prefix: prefix.to_string(),
        }
    }

    pub fn generate(&self) -> String {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("{}_{}", self.prefix, id)
    }

    pub fn generate_with_timestamp(&self) -> String {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        format!("{}_{}_{}", self.prefix, timestamp, id)
    }
}

/// Таймер для измерения длительности операций
pub struct Timer {
    start: Instant,
    name: String,
}

impl Timer {
    pub fn new(name: &str) -> Self {
        debug!("Timer '{}' started", name);
        Self {
            start: Instant::now(),
            name: name.to_string(),
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    pub fn log_elapsed(&self) {
        let elapsed = self.elapsed();
        debug!(
            "Timer '{}' elapsed: {:.3}ms",
            self.name,
            elapsed.as_secs_f64() * 1000.0
        );
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        self.log_elapsed();
    }
}

/// Кеш для хранения временных данных
#[derive(Clone)]
pub struct Cache<T: Clone> {
    data: Arc<RwLock<std::collections::HashMap<String, (T, Instant)>>>,
    ttl: Duration,
}

impl<T: Clone> Cache<T> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            data: Arc::new(RwLock::new(std::collections::HashMap::new())),
            ttl,
        }
    }

    pub async fn insert(&self, key: String, value: T) {
        let mut data = self.data.write().await;
        data.insert(key, (value, Instant::now()));
    }

    pub async fn get(&self, key: &str) -> Option<T> {
        let data = self.data.read().await;
        if let Some((value, inserted_at)) = data.get(key) {
            if inserted_at.elapsed() < self.ttl {
                return Some(value.clone());
            }
        }
        None
    }

    pub async fn remove(&self, key: &str) -> Option<T> {
        let mut data = self.data.write().await;
        data.remove(key).map(|(value, _)| value)
    }

    pub async fn cleanup_expired(&self) {
        let mut data = self.data.write().await;
        data.retain(|_, (_, inserted_at)| inserted_at.elapsed() < self.ttl);
    }

    pub async fn clear(&self) {
        let mut data = self.data.write().await;
        data.clear();
    }

    pub async fn len(&self) -> usize {
        let data = self.data.read().await;
        data.len()
    }

    pub async fn is_empty(&self) -> bool {
        let data = self.data.read().await;
        data.is_empty()
    }
}

/// Форматирование байтов в человекочитаемый вид
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    format!("{:.2} {}", size, UNITS[unit_index])
}

/// Форматирование длительности в человекочитаемый вид
pub fn format_duration(duration: Duration) -> String {
    let total_secs = duration.as_secs();
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if days > 0 {
        format!("{}d {}h {}m {}s", days, hours, minutes, seconds)
    } else if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

/// Ограничитель скорости (rate limiter)
pub struct RateLimiter {
    max_requests: usize,
    window: Duration,
    requests: Arc<RwLock<Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            requests: Arc::new(RwLock::new(Vec::new())),
        }
    }

    pub async fn check_rate_limit(&self) -> bool {
        let mut requests = self.requests.write().await;
        let now = Instant::now();

        // Удаляем старые запросы за пределами окна
        requests.retain(|&time| now.duration_since(time) < self.window);

        if requests.len() < self.max_requests {
            requests.push(now);
            true
        } else {
            false
        }
    }

    pub async fn reset(&self) {
        let mut requests = self.requests.write().await;
        requests.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_counter() {
        let counter = MetricsCounter::new();
        counter.increment_packets_received(10);
        counter.increment_packets_sent(5);
        assert_eq!(counter.get_packets_received(), 10);
        assert_eq!(counter.get_packets_sent(), 5);
    }

    #[test]
    fn test_id_generator() {
        let gen = IdGenerator::new("peer");
        let id1 = gen.generate();
        let id2 = gen.generate();
        assert_ne!(id1, id2);
        assert!(id1.starts_with("peer_"));
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(500), "500.00 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1048576), "1.00 MB");
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(90)), "1m 30s");
        assert_eq!(format_duration(Duration::from_secs(3661)), "1h 1m 1s");
    }

    #[tokio::test]
    async fn test_cache() {
        let cache = Cache::new(Duration::from_secs(1));
        cache.insert("key1".to_string(), "value1".to_string()).await;
        assert_eq!(cache.get("key1").await, Some("value1".to_string()));

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(cache.get("key1").await, None);
    }

    #[tokio::test]
    async fn test_rate_limiter() {
        let limiter = RateLimiter::new(2, Duration::from_secs(1));
        assert!(limiter.check_rate_limit().await);
        assert!(limiter.check_rate_limit().await);
        assert!(!limiter.check_rate_limit().await);

        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(limiter.check_rate_limit().await);
    }
}
