package settings

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/Madokana127/alkanet/internal/config"
)

var managedEnv = []string{
	"ANTHROPIC_BASE_URL",
	"ANTHROPIC_AUTH_TOKEN",
	"ANTHROPIC_API_KEY",
	"ANTHROPIC_MODEL",
	"ANTHROPIC_SMALL_FAST_MODEL",
	"ANTHROPIC_DEFAULT_OPUS_MODEL",
	"ANTHROPIC_DEFAULT_SONNET_MODEL",
	"ANTHROPIC_DEFAULT_HAIKU_MODEL",
	"ANTHROPIC_DEFAULT_FABLE_MODEL",
	"CLAUDE_CODE_SUBAGENT_MODEL",
	"CLAUDE_CODE_EFFORT_LEVEL",
	"CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
}

type previousState struct {
	Env      map[string]any `json:"env"`
	HasModel bool           `json:"has_model"`
	Model    any            `json:"model"`
}

func Read(path string) (map[string]any, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return map[string]any{}, nil
		}
		return nil, fmt.Errorf("读取 %s: %w", path, err)
	}
	if strings.TrimSpace(string(b)) == "" {
		return map[string]any{}, nil
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		return nil, fmt.Errorf("%s 不是合法 JSON，请先修好它: %w", path, err)
	}
	return m, nil
}

func BaseURL(m map[string]any) string {
	env, ok := m["env"].(map[string]any)
	if !ok {
		return ""
	}
	s, _ := env["ANTHROPIC_BASE_URL"].(string)
	return strings.TrimRight(s, "/")
}

func EnsureRouterToken(p config.Paths) (string, error) {
	if err := p.EnsureHome(); err != nil {
		return "", err
	}
	if b, err := os.ReadFile(p.RouterToken); err == nil {
		tok := strings.TrimSpace(string(b))
		if len(tok) >= 43 {
			_ = os.Chmod(p.RouterToken, 0o600)
			return tok, nil
		}
	}
	buf := make([]byte, 32)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	tok := base64.RawURLEncoding.EncodeToString(buf)
	if err := config.WriteAtomic(p.RouterToken, []byte(tok+"\n"), 0o600); err != nil {
		return "", err
	}
	return tok, nil
}

func backup(path string) error {
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return nil
		}
		return err
	}
	return config.WriteAtomic(path+".alkanet.bak", b, 0o600)
}

func isRouterModel(m string) bool {
	return strings.HasPrefix(m, "alkanet-") || strings.HasPrefix(m, "ccms-") || strings.HasPrefix(m, "ccx-")
}

func looksLikeRouterState(root map[string]any) bool {
	env, ok := root["env"].(map[string]any)
	if !ok {
		return false
	}
	base, _ := env["ANTHROPIC_BASE_URL"].(string)
	local := strings.HasPrefix(base, "http://127.0.0.1:") ||
		strings.HasPrefix(base, "http://localhost:") ||
		strings.HasPrefix(base, "http://[::1]:")
	_, hasToken := env["ANTHROPIC_AUTH_TOKEN"].(string)
	m1, _ := env["ANTHROPIC_MODEL"].(string)
	m2, _ := env["ANTHROPIC_SMALL_FAST_MODEL"].(string)
	return local && hasToken && (isRouterModel(m1) || isRouterModel(m2))
}

func Enable(p config.Paths, base, effort, defaultTier, token string) error {
	root, err := Read(p.Settings)
	if err != nil {
		return err
	}
	if rawEnv, exists := root["env"]; exists {
		if _, ok := rawEnv.(map[string]any); !ok {
			return fmt.Errorf("%s 的 env 不是 JSON 对象，拒绝覆盖", p.Settings)
		}
	}
	if err := backup(p.Settings); err != nil {
		return fmt.Errorf("备份 %s 失败: %w", p.Settings, err)
	}

	_, prevErr := os.Stat(p.SettingsPrevious)
	createdPrev := os.IsNotExist(prevErr) && !looksLikeRouterState(root)
	if createdPrev {
		prev := previousState{Env: map[string]any{}}
		if env, ok := root["env"].(map[string]any); ok {
			for _, k := range managedEnv {
				if v, ok := env[k]; ok {
					prev.Env[k] = v
				}
			}
		}
		if m, ok := root["model"]; ok {
			prev.HasModel = true
			prev.Model = m
		}
		pb, _ := json.MarshalIndent(prev, "", "  ")
		pb = append(pb, '\n')
		if err := config.WriteAtomic(p.SettingsPrevious, pb, 0o600); err != nil {
			return fmt.Errorf("保存前状态失败: %w", err)
		}
	}

	env, ok := root["env"].(map[string]any)
	if !ok {
		env = map[string]any{}
		root["env"] = env
	}
	delete(env, "ANTHROPIC_API_KEY")
	env["ANTHROPIC_BASE_URL"] = base
	env["ANTHROPIC_AUTH_TOKEN"] = token
	env["ANTHROPIC_MODEL"] = "alkanet-" + defaultTier
	env["ANTHROPIC_SMALL_FAST_MODEL"] = "alkanet-haiku"
	for _, tier := range []string{"fable", "opus", "sonnet", "haiku"} {
		env["ANTHROPIC_DEFAULT_"+strings.ToUpper(tier)+"_MODEL"] = "alkanet-" + tier
	}
	env["CLAUDE_CODE_SUBAGENT_MODEL"] = "alkanet-subagent"
	if strings.TrimSpace(effort) != "" {
		env["CLAUDE_CODE_EFFORT_LEVEL"] = effort
	}
	env["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"] = "1"

	if m, ok := root["model"].(string); ok && (m == "" || m == "haiku") {
		root["model"] = "alkanet-" + defaultTier
	}

	_ = os.MkdirAll(filepath.Dir(p.Settings), 0o700)
	out, _ := json.MarshalIndent(root, "", "  ")
	out = append(out, '\n')
	if err := config.WriteAtomic(p.Settings, out, 0o600); err != nil {
		if createdPrev {
			_ = os.Remove(p.SettingsPrevious)
		}
		return err
	}
	return nil
}

func Disable(p config.Paths) error {
	if _, err := os.Stat(p.Settings); os.IsNotExist(err) {
		return nil
	}
	root, err := Read(p.Settings)
	if err != nil {
		return err
	}
	if rawEnv, exists := root["env"]; exists {
		if _, ok := rawEnv.(map[string]any); !ok {
			return fmt.Errorf("%s 的 env 不是 JSON 对象，拒绝覆盖", p.Settings)
		}
	}
	if err := backup(p.Settings); err != nil {
		return fmt.Errorf("备份 %s 失败: %w", p.Settings, err)
	}

	var prev *previousState
	if pb, err := os.ReadFile(p.SettingsPrevious); err == nil {
		var st previousState
		if err := json.Unmarshal(pb, &st); err != nil {
			return fmt.Errorf("%s 已损坏，请先修复或删除它: %w", p.SettingsPrevious, err)
		}
		prev = &st
	} else if !os.IsNotExist(err) {
		return err
	}

	if env, ok := root["env"].(map[string]any); ok {
		for _, k := range managedEnv {
			delete(env, k)
		}
		if len(env) == 0 {
			delete(root, "env")
		}
	}

	if prev != nil {
		if len(prev.Env) > 0 {
			env, ok := root["env"].(map[string]any)
			if !ok {
				env = map[string]any{}
				root["env"] = env
			}
			for k, v := range prev.Env {
				env[k] = v
			}
		}
		if prev.HasModel {
			root["model"] = prev.Model
		} else {
			delete(root, "model")
		}
	}

	out, _ := json.MarshalIndent(root, "", "  ")
	out = append(out, '\n')
	if err := config.WriteAtomic(p.Settings, out, 0o600); err != nil {
		return err
	}
	if prev != nil {
		if err := os.Remove(p.SettingsPrevious); err != nil && !errors.Is(err, os.ErrNotExist) {
			return err
		}
	}
	return nil
}
