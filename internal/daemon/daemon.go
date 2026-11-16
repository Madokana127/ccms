package daemon

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/Madokana127/alkanet/internal/config"
)

const DaemonBinName = "alkanetd"

func FindDaemonBin() (string, error) {
	if env := strings.TrimSpace(os.Getenv("ALKANETD_BIN")); env != "" {
		if _, err := os.Stat(env); err == nil {
			return env, nil
		}
		return "", fmt.Errorf("ALKANETD_BIN 指向不存在的文件: %s", env)
	}
	exe, err := os.Executable()
	if err == nil {
		dir := filepath.Dir(exe)
		cand := filepath.Join(dir, DaemonBinName+exeExt())
		if _, err := os.Stat(cand); err == nil {
			return cand, nil
		}
	}
	if p, err := exec.LookPath(DaemonBinName); err == nil {
		return p, nil
	}
	return "", fmt.Errorf("找不到守护进程 %s（请将它放在 aknt 同目录或 PATH 中，或设置 ALKANETD_BIN）", DaemonBinName)
}

func exeExt() string {
	if os.PathSeparator == '\\' {
		return ".exe"
	}
	return ""
}

// VaultCall 调用 `alkanetd vault ...` 并返回 stdout。
func VaultCall(p config.Paths, stdinData string, args ...string) ([]byte, error) {
	bin, err := FindDaemonBin()
	if err != nil {
		return nil, err
	}
	full := append([]string{"vault"}, args...)
	cmd := exec.Command(bin, full...)
	if stdinData != "" {
		cmd.Stdin = strings.NewReader(stdinData)
	}
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		msg = strings.TrimPrefix(msg, "✗ ")
		if msg == "" {
			msg = err.Error()
		}
		return nil, errors.New(msg)
	}
	return stdout.Bytes(), nil
}

func localHTTPClient(timeout time.Duration) *http.Client {
	return &http.Client{
		Timeout: timeout,
		Transport: &http.Transport{
			Proxy: nil,
			DialContext: (&net.Dialer{
				Timeout: 1500 * time.Millisecond,
			}).DialContext,
		},
	}
}

func Health(listen string) (map[string]any, error) {
	client := localHTTPClient(1500 * time.Millisecond)
	resp, err := client.Get("http://" + listen + "/alkanet/health")
	if err != nil {
		return nil, nil
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return nil, nil
	}
	var m map[string]any
	if err := json.NewDecoder(resp.Body).Decode(&m); err != nil {
		return nil, nil
	}
	return m, nil
}

func IsHealthy(h map[string]any) bool {
	if h == nil {
		return false
	}
	ok, _ := h["ok"].(bool)
	svc, _ := h["service"].(string)
	return ok && (svc == "alkanet" || svc == "ccms")
}

func Alive(listen string) bool {
	h, err := Health(listen)
	return err == nil && IsHealthy(h)
}

func WaitAlive(listen string, timeout time.Duration) bool {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		if Alive(listen) {
			return true
		}
		time.Sleep(120 * time.Millisecond)
	}
	return false
}

func ReadPID(path string) (int, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return 0, nil
		}
		return 0, fmt.Errorf("读取 pidfile 失败: %w", err)
	}
	s := strings.TrimSpace(string(b))
	pid, err := strconv.Atoi(s)
	if err != nil || pid <= 0 || pid > 2147483647 {
		return 0, fmt.Errorf("pidfile 内容非法: %q（%s）", s, path)
	}
	return pid, nil
}

func Spawn(p config.Paths) error {
	rotateLog(p.Log, 5*1024*1024)
	_ = os.MkdirAll(filepath.Dir(p.Log), 0o700)
	bin, err := FindDaemonBin()
	if err != nil {
		return err
	}
	logFile, err := os.OpenFile(p.Log, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		return fmt.Errorf("打开 %s 失败: %w", p.Log, err)
	}
	defer logFile.Close()

	cmd := exec.Command(bin, "run")
	cmd.Stdin = nil
	cmd.Stdout = logFile
	cmd.Stderr = logFile
	setDetachedAttr(cmd)

	if err := cmd.Start(); err != nil {
		return fmt.Errorf("启动 router 失败: %w", err)
	}
	_ = cmd.Process.Release()
	return nil
}

func Stop(p config.Paths, listen string) (bool, error) {
	h, _ := Health(listen)
	filePID, err := ReadPID(p.PID)
	if err != nil {
		return false, err
	}
	if !IsHealthy(h) {
		if filePID != 0 {
			_ = os.Remove(p.PID)
		}
		return false, nil
	}
	if filePID == 0 {
		return false, errors.New("router 健康响应存在但缺少 pidfile，拒绝发送停止信号")
	}
	hp, ok := h["pid"].(float64)
	if !ok || int(hp) != filePID {
		return false, fmt.Errorf("health pid=%v 与 pidfile pid=%d 不一致，拒绝发送停止信号", h["pid"], filePID)
	}

	if err := killPID(filePID, false); err != nil {
		return false, err
	}
	deadline := time.Now().Add(4 * time.Second)
	for time.Now().Before(deadline) {
		if !Alive(listen) {
			_ = os.Remove(p.PID)
			return true, nil
		}
		time.Sleep(120 * time.Millisecond)
	}
	_ = killPID(filePID, true)
	time.Sleep(400 * time.Millisecond)
	_ = os.Remove(p.PID)
	return !Alive(listen), nil
}

func rotateLog(path string, maxBytes int64) {
	if fi, err := os.Stat(path); err == nil && fi.Size() > maxBytes {
		_ = os.Rename(path, path+".1")
	}
}

func TailLog(path string, lines int) (string, error) {
	if lines < 1 {
		lines = 1
	}
	f, err := os.Open(path)
	if err != nil {
		if os.IsNotExist(err) {
			return "", nil
		}
		return "", err
	}
	defer f.Close()
	fi, err := f.Stat()
	if err != nil {
		return "", err
	}
	const chunk int64 = 8192
	const maxBuf = 1024 * 1024
	pos := fi.Size()
	var buf []byte
	nl := 0
	for pos > 0 && len(buf) < maxBuf && nl < lines {
		r := chunk
		if pos < r {
			r = pos
		}
		if int64(maxBuf-len(buf)) < r {
			r = int64(maxBuf - len(buf))
		}
		if r <= 0 {
			break
		}
		pos -= r
		if _, err := f.Seek(pos, io.SeekStart); err != nil {
			return "", err
		}
		c := make([]byte, r)
		if _, err := io.ReadFull(f, c); err != nil {
			return "", err
		}
		for _, b := range c {
			if b == '\n' {
				nl++
			}
		}
		buf = append(c, buf...)
	}
	all := strings.Split(strings.TrimRight(string(buf), "\n"), "\n")
	if len(all) == 1 && all[0] == "" {
		return "", nil
	}
	if len(all) > lines {
		all = all[len(all)-lines:]
	}
	return strings.Join(all, "\n"), nil
}
