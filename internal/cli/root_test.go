package cli_test

import (
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func buildBins(t *testing.T) (string, string) {
	t.Helper()
	root, err := filepath.Abs("../..")
	if err != nil {
		t.Fatal(err)
	}
	aknt := filepath.Join(t.TempDir(), "aknt")
	b := exec.Command("go", "build", "-o", aknt, "./cmd/aknt")
	b.Dir = root
	if out, err := b.CombinedOutput(); err != nil {
		t.Fatalf("build aknt: %v\n%s", err, out)
	}
	rust := exec.Command("cargo", "build", "--bin", "alkanetd")
	rust.Dir = root
	if out, err := rust.CombinedOutput(); err != nil {
		t.Fatalf("build alkanetd: %v\n%s", err, out)
	}
	alkanetd := filepath.Join(root, "target", "debug", "alkanetd")
	return aknt, alkanetd
}

func runCLI(t *testing.T, aknt, alkanetd, home, settings string, args ...string) (string, string, error) {
	t.Helper()
	cmd := exec.Command(aknt, args...)
	cmd.Env = append(os.Environ(),
		"ALKANET_HOME="+home,
		"CLAUDE_SETTINGS="+settings,
		"ALKANETD_BIN="+alkanetd,
		"NO_COLOR=1",
	)
	var out, errb strings.Builder
	cmd.Stdout = &out
	cmd.Stderr = &errb
	err := cmd.Run()
	return out.String(), errb.String(), err
}

func freePort(t *testing.T) int {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

func TestCLISurfaceAndLifecycle(t *testing.T) {
	aknt, alkanetd := buildBins(t)
	tmp := t.TempDir()
	home := filepath.Join(tmp, "alkanet")
	settings := filepath.Join(tmp, ".claude", "settings.json")
	if err := os.MkdirAll(filepath.Dir(settings), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(settings, []byte(`{"env":{"ANTHROPIC_API_KEY":"sk-official","KEEP":"1"}}`), 0o600); err != nil {
		t.Fatal(err)
	}

	// 1. version & help
	out, _, err := runCLI(t, aknt, alkanetd, home, settings, "version")
	if err != nil || !strings.Contains(out, "aknt 1.0.0") {
		t.Fatalf("version failed: %v %q", err, out)
	}
	out, _, err = runCLI(t, aknt, alkanetd, home, settings, "--help")
	if err != nil || !strings.Contains(out, "aknt") {
		t.Fatalf("help failed: %v %q", err, out)
	}

	// 2. bare invocation initializes config.toml & shows dashboard
	out, errOut, err := runCLI(t, aknt, alkanetd, home, settings)
	if err != nil {
		t.Fatalf("dashboard failed: %v\n%s", err, errOut)
	}
	if !strings.HasPrefix(out, "Mode ") || !strings.Contains(out, "Route List") || !strings.Contains(out, "提示  aknt --help") {
		t.Fatalf("unexpected dashboard:\n%s", out)
	}
	if _, err := os.Stat(filepath.Join(home, "config.toml")); err != nil {
		t.Fatalf("config.toml not initialized: %v", err)
	}

	// 3. use switches all 5 tiers in config.toml
	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "use", "deepseek", "deepseek-v4-pro")
	if err != nil || !strings.Contains(out, "deepseek/deepseek-v4-pro") {
		t.Fatalf("use failed: %v %s %s", err, out, errOut)
	}

	// 4. key add / ls / rm / clear via alkanetd
	_, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "key", "add", "deepseek", "sk-test-1234567890")
	if err != nil {
		t.Fatalf("key add failed: %v %s", err, errOut)
	}
	out, _, err = runCLI(t, aknt, alkanetd, home, settings, "key", "ls", "deepseek")
	if err != nil || !strings.Contains(out, "sk-tes…7890") {
		t.Fatalf("key ls failed: %v %s", err, out)
	}
	_, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "key", "rm", "deepseek", "1")
	if err != nil {
		t.Fatalf("key rm failed: %v %s", err, errOut)
	}
	_, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "key", "clear", "deepseek")
	if err != nil {
		t.Fatalf("key clear failed: %v %s", err, errOut)
	}

	// 5. mock upstream + port override + on / status / test router / off / router stop
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("content-type", "application/json")
		_, _ = w.Write([]byte(`{"id":"msg_1","type":"message","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":5,"output_tokens":2}}`))
	}))
	defer upstream.Close()

	port := freePort(t)
	cfgBytes, err := os.ReadFile(filepath.Join(home, "config.toml"))
	if err != nil {
		t.Fatal(err)
	}
	patched := strings.ReplaceAll(string(cfgBytes), "127.0.0.1:8788", fmt.Sprintf("127.0.0.1:%d", port))
	patched = strings.ReplaceAll(patched, "https://api.deepseek.com/anthropic", upstream.URL)
	if err := os.WriteFile(filepath.Join(home, "config.toml"), []byte(patched), 0o600); err != nil {
		t.Fatal(err)
	}
	_, _, _ = runCLI(t, aknt, alkanetd, home, settings, "key", "add", "deepseek", "sk-mock-key-123456")

	defer func() {
		_, _, _ = runCLI(t, aknt, alkanetd, home, settings, "router", "stop")
	}()

	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "on")
	if err != nil {
		t.Fatalf("on failed: %v\n%s\n%s", err, out, errOut)
	}

	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "status", "--json")
	if err != nil {
		t.Fatalf("status --json failed: %v %s", err, errOut)
	}
	var st map[string]any
	if err := json.Unmarshal([]byte(out), &st); err != nil {
		t.Fatalf("invalid status json: %v\n%s", err, out)
	}
	if alive, _ := st["router_alive"].(bool); !alive {
		t.Fatalf("expected router_alive=true: %v", st)
	}

	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "test", "router")
	if err != nil || !strings.Contains(out, "HTTP 200") {
		t.Fatalf("test router failed: %v\n%s\n%s", err, out, errOut)
	}

	// stats should now have 1 request
	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "stats", "--json")
	if err != nil || !strings.Contains(out, `"requests": 1`) {
		t.Fatalf("stats --json failed: %v %s %s", err, out, errOut)
	}

	_, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "off")
	if err != nil {
		t.Fatalf("off failed: %v %s", err, errOut)
	}
	restored, err := os.ReadFile(settings)
	if err != nil || !strings.Contains(string(restored), "sk-official") || strings.Contains(string(restored), "alkanet-sonnet") {
		t.Fatalf("settings not restored: %s", restored)
	}

	out, errOut, err = runCLI(t, aknt, alkanetd, home, settings, "router", "stop")
	if err != nil || !strings.Contains(out, "已停止") {
		t.Fatalf("router stop failed: %v %s %s", err, out, errOut)
	}
}
