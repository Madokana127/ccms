//go:build windows

package daemon

import (
	"fmt"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
)

func setDetachedAttr(cmd *exec.Cmd) {
	const detachedProcess = 0x00000008
	const createNewProcessGroup = 0x00000200
	cmd.SysProcAttr = &syscall.SysProcAttr{
		CreationFlags: detachedProcess | createNewProcessGroup,
	}
}

func killPID(pid int, _ bool) error {
	cmd := exec.Command("taskkill", "/PID", strconv.Itoa(pid), "/F")
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: 0x08000000}
	out, err := cmd.CombinedOutput()
	if err != nil {
		return fmt.Errorf("taskkill PID %d 失败: %s", pid, strings.TrimSpace(string(out)))
	}
	return nil
}
