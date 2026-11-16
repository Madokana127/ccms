//go:build unix

package daemon

import (
	"fmt"
	"os/exec"
	"syscall"
)

func setDetachedAttr(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Setsid: true,
	}
}

func killPID(pid int, force bool) error {
	sig := syscall.SIGTERM
	name := "SIGTERM"
	if force {
		sig = syscall.SIGKILL
		name = "SIGKILL"
	}
	if err := syscall.Kill(pid, sig); err != nil {
		return fmt.Errorf("发送信号 %s 到 PID %d 失败: %w", name, pid, err)
	}
	return nil
}
