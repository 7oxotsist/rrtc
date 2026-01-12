use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::Path;
use webrtc::ice_transport::ice_credential_type::RTCIceCredentialType;
use webrtc::ice_transport::ice_server::RTCIceServer;

/// Конфигурация ICE сервера (STUN/TURN)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServerConfig {
    /// URL адреса сервера (например: stun:stun.l.google.com:19302)
    pub urls: Vec<String>,
    /// Имя пользователя для TURN
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Пароль/credential для TURN
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl IceServerConfig {
    pub fn to_rtc_ice_server(&self) -> RTCIceServer {
        RTCIceServer {
            urls: self.urls.clone(),
            username: self.username.clone().unwrap_or_default(),
            credential: self.credential.clone().unwrap_or_default(),
            credential_type: if self.credential.is_some() {
                RTCIceCredentialType::Password
            } else {
                RTCIceCredentialType::Unspecified
            },
        }
    }
}

/// Основная конфигурация SFU сервера
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Порт для WebSocket signaling
    #[serde(default = "default_signaling_port")]
    pub signaling_port: u16,

    /// Адрес для прослушивания
    #[serde(default = "default_listen_address")]
    pub listen_address: String,

    /// ICE серверы (STUN/TURN)
    #[serde(default = "default_ice_servers")]
    pub ice_servers: Vec<IceServerConfig>,

    /// Максимальное количество участников в комнате
    #[serde(default = "default_max_participants")]
    pub max_participants_per_room: usize,

    /// Таймаут для неактивных соединений (секунды)
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Включить детальное логирование
    #[serde(default = "default_verbose_logging")]
    pub verbose_logging: bool,

    /// Интервал очистки пустых комнат (секунды)
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,

    /// Поддержка TLS (для будущего использования)
    #[serde(default)]
    pub tls_enabled: bool,

    /// Путь к TLS сертификату
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_cert_path: Option<String>,

    /// Путь к TLS ключу
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_key_path: Option<String>,
}

// Значения по умолчанию
fn default_signaling_port() -> u16 {
    8080
}

fn default_listen_address() -> String {
    "0.0.0.0".to_string()
}

fn default_ice_servers() -> Vec<IceServerConfig> {
    vec![
        IceServerConfig {
            urls: vec!["stun:stun.l.google.com:19302".to_string()],
            username: None,
            credential: None,
        },
        IceServerConfig {
            urls: vec!["stun:stun1.l.google.com:19302".to_string()],
            username: None,
            credential: None,
        },
        // Coturn TURN server для NAT traversal
        IceServerConfig {
            urls: vec![
                "turn:coturn:3478?transport=udp".to_string(),
                "turn:coturn:3478?transport=tcp".to_string(),
            ],
            username: Some("webrtc".to_string()),
            credential: Some("secure_password_123".to_string()),
        },
    ]
}

fn default_max_participants() -> usize {
    50
}

fn default_connection_timeout() -> u64 {
    300 // 5 минут
}

fn default_verbose_logging() -> bool {
    false
}

fn default_cleanup_interval() -> u64 {
    60 // 1 минута
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            signaling_port: default_signaling_port(),
            listen_address: default_listen_address(),
            ice_servers: default_ice_servers(),
            max_participants_per_room: default_max_participants(),
            connection_timeout_secs: default_connection_timeout(),
            verbose_logging: default_verbose_logging(),
            cleanup_interval_secs: default_cleanup_interval(),
            tls_enabled: false,
            tls_cert_path: None,
            tls_key_path: None,
        }
    }
}

impl ServerConfig {
    /// Загружает конфигурацию из файла
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref())
            .context("Failed to read config file")?;

        let config: ServerConfig = toml::from_str(&content)
            .or_else(|_| serde_json::from_str(&content))
            .context("Failed to parse config file (tried TOML and JSON)")?;

        Ok(config)
    }

    /// Загружает конфигурацию из переменных окружения
    pub fn from_env() -> Result<Self> {
        let mut config = ServerConfig::default();

        if let Ok(port) = env::var("SIGNALING_PORT") {
            config.signaling_port = port.parse().context("Invalid SIGNALING_PORT")?;
        }

        if let Ok(addr) = env::var("LISTEN_ADDRESS") {
            config.listen_address = addr;
        }

        if let Ok(max_participants) = env::var("MAX_PARTICIPANTS") {
            config.max_participants_per_room = max_participants
                .parse()
                .context("Invalid MAX_PARTICIPANTS")?;
        }

        if let Ok(verbose) = env::var("VERBOSE_LOGGING") {
            config.verbose_logging = verbose.parse().unwrap_or(false);
        }

        // Загрузка TURN конфигурации из переменных окружения
        if let Ok(turn_url) = env::var("TURN_URL") {
            let username = env::var("TURN_USERNAME").ok();
            let credential = env::var("TURN_CREDENTIAL").ok();

            config.ice_servers.push(IceServerConfig {
                urls: vec![turn_url],
                username,
                credential,
            });
        }

        // Поддержка нескольких TURN серверов через TURN_URLS
        if let Ok(turn_urls) = env::var("TURN_URLS") {
            let urls: Vec<String> = turn_urls
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if !urls.is_empty() {
                let username = env::var("TURN_USERNAME").ok();
                let credential = env::var("TURN_CREDENTIAL").ok();

                config.ice_servers.push(IceServerConfig {
                    urls,
                    username,
                    credential,
                });
            }
        }

        // TLS настройки
        if let Ok(tls_enabled) = env::var("TLS_ENABLED") {
            config.tls_enabled = tls_enabled.parse().unwrap_or(false);
        }

        config.tls_cert_path = env::var("TLS_CERT_PATH").ok();
        config.tls_key_path = env::var("TLS_KEY_PATH").ok();

        Ok(config)
    }

    /// Загружает конфигурацию из файла или переменных окружения
    /// Приоритет: файл конфигурации -> переменные окружения -> значения по умолчанию
    pub fn load() -> Result<Self> {
        // Проверяем наличие файла конфигурации
        if let Ok(config_path) = env::var("CONFIG_FILE") {
            if Path::new(&config_path).exists() {
                return Self::from_file(config_path);
            }
        }

        // Проверяем стандартные пути
        for path in &["config.toml", "config.json", "/etc/rrtc/config.toml"] {
            if Path::new(path).exists() {
                return Self::from_file(path);
            }
        }

        // Загружаем из переменных окружения
        Self::from_env()
    }

    /// Возвращает RTCIceServer конфигурацию для WebRTC
    pub fn get_rtc_ice_servers(&self) -> Vec<RTCIceServer> {
        self.ice_servers
            .iter()
            .map(|config| config.to_rtc_ice_server())
            .collect()
    }

    /// Валидация конфигурации
    pub fn validate(&self) -> Result<()> {
        if self.signaling_port == 0 {
            anyhow::bail!("Signaling port cannot be 0");
        }

        if self.max_participants_per_room == 0 {
            anyhow::bail!("Max participants per room must be greater than 0");
        }

        if self.ice_servers.is_empty() {
            anyhow::bail!("At least one ICE server must be configured");
        }

        if self.tls_enabled {
            if self.tls_cert_path.is_none() || self.tls_key_path.is_none() {
                anyhow::bail!("TLS enabled but cert/key paths not provided");
            }
        }

        Ok(())
    }

    /// Сохраняет конфигурацию в файл
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content = if path.as_ref().extension().and_then(|s| s.to_str()) == Some("json") {
            serde_json::to_string_pretty(self)?
        } else {
            toml::to_string_pretty(self)?
        };

        fs::write(path, content)?;
        Ok(())
    }
}

/// Конфигурация для конкретной комнаты (расширенная)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomConfig {
    pub id: String,

    /// Максимальное количество участников в этой комнате
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_participants: Option<usize>,

    /// Требуется ли пароль для входа
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,

    /// Разрешен ли screen sharing
    #[serde(default = "default_true")]
    pub screen_sharing_enabled: bool,

    /// Запись включена
    #[serde(default)]
    pub recording_enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Default for RoomConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            max_participants: None,
            password: None,
            screen_sharing_enabled: true,
            recording_enabled: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ServerConfig::default();
        assert_eq!(config.signaling_port, 8080);
        assert_eq!(config.listen_address, "0.0.0.0");
        assert!(!config.ice_servers.is_empty());
    }

    #[test]
    fn test_ice_server_conversion() {
        let ice_config = IceServerConfig {
            urls: vec!["stun:stun.example.com:3478".to_string()],
            username: Some("user".to_string()),
            credential: Some("pass".to_string()),
        };

        let rtc_server = ice_config.to_rtc_ice_server();
        assert_eq!(rtc_server.urls.len(), 1);
        assert_eq!(rtc_server.username, "user");
    }

    #[test]
    fn test_config_validation() {
        let config = ServerConfig::default();
        assert!(config.validate().is_ok());

        let mut invalid_config = config.clone();
        invalid_config.signaling_port = 0;
        assert!(invalid_config.validate().is_err());
    }
}
