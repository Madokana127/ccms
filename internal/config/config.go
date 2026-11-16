package config

import (
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	toml "github.com/pelletier/go-toml/v2"
)

//go:embed default.toml
var DefaultTOML string

var Tiers = []string{"fable", "opus", "sonnet", "haiku", "subagent"}

func TierLabel(t string) string {
	switch t {
	case "fable":
		return "Fable"
	case "opus":
		return "Opus"
	case "sonnet":
		return "Sonnet"
	case "haiku":
		return "Haiku"
	case "subagent":
		return "subagent"
	default:
		return t
	}
}

func IsTier(t string) bool {
	for _, x := range Tiers {
		if x == t {
			return true
		}
	}
	return false
}

type Paths struct {
	Home             string
	Config           string
	Routes           string
	Providers        string
	KeysEnv          string
	KeysEnc          string
	Keyfile          string
	Stats            string
	Log              string
	PID              string
	RouterToken      string
	Settings         string
	SettingsPrevious string
}

func LoadPaths() (Paths, error) {
	home := envPath("ALKANET_HOME")
	if home == "" {
		home = envPath("CCMS_HOME")
	}
	if home == "" {
		hd, err := homeDir()
		if err != nil {
			return Paths{}, fmt.Errorf("无法确定用户主目录：请设置 ALKANET_HOME 或 HOME/USERPROFILE；拒绝回退到当前目录")
		}
		home = filepath.Join(hd, ".config", "alkanet")
	}

	settings := envPath("CLAUDE_SETTINGS")
	if settings == "" {
		hd, err := homeDir()
		if err != nil {
			return Paths{}, fmt.Errorf("无法确定 Claude 设置路径：请设置 CLAUDE_SETTINGS 或 HOME/USERPROFILE；拒绝回退到当前目录")
		}
		settings = filepath.Join(hd, ".claude", "settings.json")
	}

	return Paths{
		Home:             home,
		Config:           filepath.Join(home, "config.toml"),
		Routes:           filepath.Join(home, "routes.json"),
		Providers:        filepath.Join(home, "providers.json"),
		KeysEnv:          filepath.Join(home, "keys.env"),
		KeysEnc:          filepath.Join(home, "keys.enc"),
		Keyfile:          filepath.Join(home, "keystore.key"),
		Stats:            filepath.Join(home, "stats.json"),
		Log:              filepath.Join(home, "router.log"),
		PID:              filepath.Join(home, "router.pid"),
		RouterToken:      filepath.Join(home, "router.token"),
		Settings:         settings,
		SettingsPrevious: filepath.Join(home, "settings.previous.json"),
	}, nil
}

func (p Paths) EnsureHome() error {
	if err := os.MkdirAll(p.Home, 0o700); err != nil {
		return err
	}
	_ = os.Chmod(p.Home, 0o700)
	return nil
}

func homeDir() (string, error) {
	for _, k := range []string{"HOME", "USERPROFILE"} {
		if v := envPath(k); v != "" {
			return v, nil
		}
	}
	return "", errors.New("no home")
}

func envPath(k string) string {
	return strings.TrimSpace(os.Getenv(k))
}

type Route struct {
	Provider string `toml:"provider" json:"provider"`
	Model    string `toml:"model" json:"model"`
}

type Retry struct {
	Attempts          uint32   `toml:"attempts" json:"attempts"`
	BackoffMs         uint64   `toml:"backoff_ms" json:"backoff_ms"`
	ResponseTimeoutMs uint64   `toml:"response_timeout_ms" json:"response_timeout_ms"`
	ConnectTimeoutMs  uint64   `toml:"connect_timeout_ms" json:"connect_timeout_ms"`
	RetryOn           []uint16 `toml:"retry_on" json:"retry_on"`
}

type Provider struct {
	Label        string   `toml:"label" json:"label,omitempty"`
	BaseURL      string   `toml:"base_url" json:"base_url"`
	KeyEnv       string   `toml:"key_env" json:"key_env,omitempty"`
	KeyEnvPool   string   `toml:"key_env_pool" json:"key_env_pool,omitempty"`
	DefaultModel string   `toml:"default_model" json:"default_model,omitempty"`
	Model        string   `toml:"model" json:"model,omitempty"`
	Models       []string `toml:"models" json:"models,omitempty"`
	Auth         string   `toml:"auth" json:"auth,omitempty"`
	Note         string   `toml:"note" json:"note,omitempty"`
}

func (p Provider) EffectiveModel() string {
	if p.DefaultModel != "" {
		return p.DefaultModel
	}
	if p.Model != "" {
		return p.Model
	}
	if len(p.Models) > 0 {
		return p.Models[0]
	}
	return ""
}

func (p Provider) Display(name string) string {
	if p.Label != "" {
		return p.Label
	}
	return name
}

func (p Provider) UsesXAPIKey() bool {
	return p.Auth == "x-api-key" || p.Auth == "x_api_key"
}

type AppConfig struct {
	Listen    string              `toml:"listen" json:"listen"`
	Default   string              `toml:"default" json:"default"`
	Effort    string              `toml:"effort" json:"effort"`
	Retry     Retry               `toml:"retry" json:"retry"`
	Routes    map[string]Route    `toml:"routes" json:"routes"`
	Providers map[string]Provider `toml:"providers" json:"providers"`
}

func EnsureDefaults(p Paths) error {
	if err := p.EnsureHome(); err != nil {
		return fmt.Errorf("创建 ALKANET_HOME 失败: %w", err)
	}
	_, errToml := os.Stat(p.Config)
	_, errRoutes := os.Stat(p.Routes)
	if os.IsNotExist(errToml) && os.IsNotExist(errRoutes) {
		return WriteAtomic(p.Config, []byte(DefaultTOML), 0o600)
	}
	return nil
}

func ResetDefaultTOML(p Paths) error {
	if err := p.EnsureHome(); err != nil {
		return err
	}
	return WriteAtomic(p.Config, []byte(DefaultTOML), 0o600)
}

func Load(p Paths) (AppConfig, error) {
	if err := EnsureDefaults(p); err != nil {
		return AppConfig{}, err
	}
	if data, err := os.ReadFile(p.Config); err == nil {
		var cfg AppConfig
		if err := toml.Unmarshal(data, &cfg); err != nil {
			return AppConfig{}, fmt.Errorf("%s 不是合法的 TOML 配置: %w", p.Config, err)
		}
		applyDefaults(&cfg)
		return cfg, nil
	} else if !os.IsNotExist(err) {
		return AppConfig{}, err
	}

	// 兼容旧版 routes.json + providers.json
	var cfg AppConfig
	rdata, err := os.ReadFile(p.Routes)
	if err != nil {
		return AppConfig{}, fmt.Errorf("读取配置失败: %w", err)
	}
	if err := json.Unmarshal(rdata, &cfg); err != nil {
		return AppConfig{}, fmt.Errorf("%s 不是合法 JSON: %w", p.Routes, err)
	}
	pdata, err := os.ReadFile(p.Providers)
	if err != nil {
		return AppConfig{}, fmt.Errorf("读取 %s 失败: %w", p.Providers, err)
	}
	if err := json.Unmarshal(pdata, &cfg.Providers); err != nil {
		return AppConfig{}, fmt.Errorf("%s 不是合法 JSON: %w", p.Providers, err)
	}
	applyDefaults(&cfg)
	return cfg, nil
}

func SaveTOML(p Paths, cfg AppConfig) error {
	data, err := toml.Marshal(cfg)
	if err != nil {
		return err
	}
	return WriteAtomic(p.Config, data, 0o600)
}

func applyDefaults(cfg *AppConfig) {
	if cfg.Listen == "" {
		cfg.Listen = "127.0.0.1:8788"
	}
	if cfg.Default == "" {
		cfg.Default = "sonnet"
	}
	if cfg.Effort == "" {
		cfg.Effort = "high"
	}
	if cfg.Routes == nil {
		cfg.Routes = map[string]Route{}
	}
	if cfg.Providers == nil {
		cfg.Providers = map[string]Provider{}
	}
}

func WriteAtomic(path string, data []byte, mode os.FileMode) error {
	dir := filepath.Dir(path)
	if dir != "" && dir != "." {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			return err
		}
	}
	tmp := fmt.Sprintf("%s.tmp.%d", path, os.Getpid())
	if err := os.WriteFile(tmp, data, mode); err != nil {
		return err
	}
	_ = os.Chmod(tmp, mode)
	if err := os.Rename(tmp, path); err != nil {
		_ = os.Remove(tmp)
		return err
	}
	return nil
}
