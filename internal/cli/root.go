package cli

import (
	"bufio"
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/spf13/cobra"

	"github.com/Madokana127/alkanet/internal/config"
	"github.com/Madokana127/alkanet/internal/daemon"
	"github.com/Madokana127/alkanet/internal/settings"
	"github.com/Madokana127/alkanet/internal/stats"
	"github.com/Madokana127/alkanet/internal/term"
)

const Version = "1.0.0"

func NewRootCmd() *cobra.Command {
	root := &cobra.Command{
		Use:                "aknt",
		Short:              "Alkanet — Claude Code 多供应商前置路由器",
		SilenceUsage:       true,
		SilenceErrors:      true,
		DisableFlagParsing: true,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) > 0 {
				return fmt.Errorf("未知命令: %s（执行 aknt help 查看用法）", args[0])
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			return runDashboard(p)
		},
	}

	root.SetHelpCommand(&cobra.Command{Hidden: true})
	root.SetHelpFunc(func(cmd *cobra.Command, args []string) {
		printFullHelp()
	})

	root.AddCommand(
		cmdHelp(),
		cmdVersion(),
		cmdOn(),
		cmdOff(),
		cmdStatus(),
		cmdUse(),
		cmdConfig(),
		cmdKey(),
		cmdRouter(),
		cmdTest(),
		cmdModels(),
		cmdStats(),
		cmdCompletion(root),
	)
	return root
}

func rejectFlags(args []string) error {
	for _, a := range args {
		if strings.HasPrefix(a, "-") {
			return fmt.Errorf("不支持短横线参数 %s，请使用裸命令（执行 aknt help 查看用法）", a)
		}
	}
	return nil
}

func cmdHelp() *cobra.Command {
	return &cobra.Command{
		Use:                "help",
		Short:              "显示完整命令帮助",
		DisableFlagParsing: true,
		Run: func(cmd *cobra.Command, args []string) {
			printFullHelp()
		},
	}
}

func printFullHelp() {
	s := term.Detect()
	fmt.Printf("%s — Claude Code 多供应商前置路由器  v%s\n\n", s.Bold("Alkanet (aknt)"), Version)

	section := func(title string) {
		fmt.Println(s.Bold(title))
	}
	item := func(indent int, cmd, desc string) {
		pad := strings.Repeat(" ", indent)
		width := 28 - indent
		if width < len(cmd)+2 {
			width = len(cmd) + 2
		}
		fmt.Printf("%s%-*s%s\n", pad, width, cmd, desc)
	}

	section("Usage")
	item(4, "aknt", "显示当前模式、路由与密钥看板")
	item(4, "aknt <command> ...", "执行指定子命令")
	fmt.Println()

	section("Mode & Status")
	item(4, "on", "启用路由模式（拉起守护进程并接管 settings.json）")
	item(4, "off", "关闭路由模式（恢复 Claude settings.json）")
	item(4, "status", "查看运行状态与密钥库")
	item(8, "check", "附带环境、配置与文件权限完整自检")
	item(8, "json", "以 JSON 格式输出状态（可与 check 组合）")
	item(4, "version", "显示版本信息")
	item(4, "help", "显示完整命令序列")
	fmt.Println()

	section("Config & Routing")
	item(4, "use <provider> [model]", "将五个档位统一切换到指定 provider")
	item(4, "config", "查看当前生效配置、路由与 provider 列表")
	item(8, "edit", "用 $EDITOR 打开 config.toml（保存后校验）")
	item(8, "path", "输出 config.toml 文件路径")
	item(8, "reset force", "重置 config.toml 为内置默认")
	fmt.Println()

	section("Key & Vault")
	item(4, "key", "管理加密密钥池与密钥库")
	item(8, "add <provider> <key>", "向密钥池添加一把 key")
	item(8, "add <provider> stdin", "从标准输入读取一把或多把 key（推荐）")
	item(8, "ls <provider>", "列出某 provider 的脱敏密钥池")
	item(8, "rm <provider> <n>", "移除第 n 把 key")
	item(8, "clear <provider>", "清空某 provider 的密钥池")
	item(8, "seal", "将明文 keys.env 迁移加密为 keys.enc")
	item(8, "rekey", "轮换主密钥或切换密钥库模式")
	fmt.Println()

	section("Router Daemon")
	item(4, "router", "管理守护进程 alkanetd")
	item(8, "start", "启动 alkanetd")
	item(8, "stop", "停止 alkanetd")
	item(8, "restart", "重启 alkanetd")
	item(8, "logs [n]", "查看最近 n 行日志（默认 40）")
	fmt.Println()

	section("Diagnostics & Stats")
	item(4, "models <provider>", "拉取 provider 可用模型列表")
	item(4, "test [target]", "验证链路（router | all | <provider>）")
	item(4, "stats [days]", "查看用量统计（默认 7 天）")
	item(8, "by <dim>", "按维度过滤（model | provider | tier）")
	item(8, "json", "以 JSON 格式输出统计")
	item(8, "reset", "清空统计数据（需确认）")
	fmt.Println()

	section("Shell")
	item(4, "completion <shell>", "生成补全脚本（bash | zsh | fish | powershell）")
}

func cmdVersion() *cobra.Command {
	return &cobra.Command{
		Use:                "version",
		Short:              "显示版本",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		Run: func(cmd *cobra.Command, args []string) {
			fmt.Printf("aknt %s — Alkanet (CLI + alkanetd)\n", Version)
		},
	}
}

func cmdOn() *cobra.Command {
	return &cobra.Command{
		Use:                "on",
		Short:              "启用路由模式",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			cfg, err := config.Load(p)
			if err != nil {
				return err
			}
			if _, ok := cfg.Routes[cfg.Default]; !ok {
				return fmt.Errorf("默认档位 %s 未配置路由（用 aknt use <provider> 或编辑 %s）", cfg.Default, p.Config)
			}
			s := term.Detect()
			if !daemon.Alive(cfg.Listen) {
				if err := daemon.Spawn(p); err != nil {
					return err
				}
				if !daemon.WaitAlive(cfg.Listen, 8*time.Second) {
					return fmt.Errorf("router 启动失败，查看 aknt router logs（%s）", p.Log)
				}
				fmt.Println(s.Ok("router 已启动 http://" + cfg.Listen))
			}
			tok, err := settings.EnsureRouterToken(p)
			if err != nil {
				return err
			}
			base := "http://" + cfg.Listen
			if err := settings.Enable(p, base, cfg.Effort, cfg.Default, tok); err != nil {
				return err
			}
			fmt.Println(s.Ok("路由模式已启用"))
			fmt.Println(s.Dim("新开一个 claude 会话生效；关闭用 aknt off"))
			return nil
		},
	}
}

func cmdOff() *cobra.Command {
	return &cobra.Command{
		Use:                "off",
		Short:              "关闭路由模式",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			if err := settings.Disable(p); err != nil {
				return err
			}
			s := term.Detect()
			fmt.Println(s.Ok("已关闭路由，恢复 Claude 原设置"))
			fmt.Println(s.Dim("router 进程如仍在运行可保留，或执行 aknt router stop"))
			return nil
		},
	}
}

func cmdStatus() *cobra.Command {
	return &cobra.Command{
		Use:                "status [check] [json]",
		Short:              "查看运行状态与环境健康",
		DisableFlagParsing: true,
		Args:               cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			var asJSON, check bool
			for _, a := range args {
				switch a {
				case "json":
					asJSON = true
				case "check":
					check = true
				default:
					return fmt.Errorf("未知 status 参数: %s（可用: aknt status [check] [json]）", a)
				}
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			return runStatus(p, asJSON, check)
		},
	}
}

func cmdUse() *cobra.Command {
	return &cobra.Command{
		Use:                "use <provider> [model]",
		Short:              "将五个档位统一切换到指定 provider",
		DisableFlagParsing: true,
		Args:               cobra.RangeArgs(1, 2),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			cfg, err := config.Load(p)
			if err != nil {
				return err
			}
			provName := args[0]
			prov, ok := cfg.Providers[provName]
			if !ok {
				return fmt.Errorf("未知 provider: %s", provName)
			}
			model := ""
			if len(args) > 1 {
				model = args[1]
			} else {
				model = prov.EffectiveModel()
			}
			if model == "" {
				return fmt.Errorf("%s 没有 default_model，请显式指定模型", provName)
			}
			for _, t := range config.Tiers {
				cfg.Routes[t] = config.Route{Provider: provName, Model: model}
			}
			if err := config.SaveTOML(p, cfg); err != nil {
				return err
			}
			s := term.Detect()
			fmt.Println(s.Ok(fmt.Sprintf("五个档位已统一到 %s/%s（热加载生效）", provName, model)))
			return nil
		},
	}
}

func cmdConfig() *cobra.Command {
	c := &cobra.Command{
		Use:                "config",
		Short:              "管理配置",
		DisableFlagParsing: true,
		Args:               cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if len(args) > 0 {
				return fmt.Errorf("未知 config 操作: %s（可用: edit | path | reset force）", args[0])
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			cfg, err := config.Load(p)
			if err != nil {
				return err
			}
			s := term.Detect()
			fmt.Println(s.Section("配置"))
			fmt.Printf("  config.toml  %s\n", p.Config)
			fmt.Printf("  listen       %s\n", cfg.Listen)
			fmt.Printf("  default      %s  effort=%s\n", cfg.Default, cfg.Effort)
			printRoutes(cfg, s)
			fmt.Println(s.Section("Providers"))
			names := sortedProviderNames(cfg.Providers)
			for _, name := range names {
				pr := cfg.Providers[name]
				fmt.Printf("  %-12s %-20s %s (%s)\n", name, pr.Display(name), pr.BaseURL, pr.EffectiveModel())
			}
			return nil
		},
	}
	c.AddCommand(
		&cobra.Command{
			Use:                "path",
			Short:              "输出配置文件路径",
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				if err := config.EnsureDefaults(p); err != nil {
					return err
				}
				fmt.Println(p.Config)
				return nil
			},
		},
		&cobra.Command{
			Use:                "edit",
			Short:              "用 $EDITOR 打开 config.toml",
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				if err := config.EnsureDefaults(p); err != nil {
					return err
				}
				ed := strings.TrimSpace(os.Getenv("EDITOR"))
				if ed == "" {
					if os.PathSeparator == '\\' {
						ed = "notepad"
					} else {
						ed = "vi"
					}
				}
				ec := exec.Command(ed, p.Config)
				ec.Stdin = os.Stdin
				ec.Stdout = os.Stdout
				ec.Stderr = os.Stderr
				if err := ec.Run(); err != nil {
					return err
				}
				if _, err := config.Load(p); err != nil {
					return fmt.Errorf("保存的配置校验失败: %w", err)
				}
				fmt.Println(term.Detect().Ok("配置有效，router 将自动热加载"))
				return nil
			},
		},
		&cobra.Command{
			Use:                "reset force",
			Short:              "重置 config.toml 为内置默认",
			DisableFlagParsing: true,
			Args:               cobra.MaximumNArgs(1),
			RunE: func(cmd *cobra.Command, args []string) error {
				if err := rejectFlags(args); err != nil {
					return err
				}
				if len(args) != 1 || args[0] != "force" {
					return errors.New("重置会覆盖现有 config.toml，请执行: aknt config reset force")
				}
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				if err := config.ResetDefaultTOML(p); err != nil {
					return err
				}
				fmt.Println(term.Detect().Ok("已重置默认配置: " + p.Config))
				return nil
			},
		},
	)
	return c
}

func cmdKey() *cobra.Command {
	k := &cobra.Command{
		Use:                "key",
		Short:              "管理加密密钥池与密钥库",
		DisableFlagParsing: true,
	}

	addCmd := &cobra.Command{
		Use:                "add <provider> <key|stdin>",
		Short:              "向密钥池添加 key",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			provider := args[0]
			if strings.HasPrefix(provider, "-") {
				return fmt.Errorf("不支持短横线参数 %s，请使用裸命令（执行 aknt help 查看用法）", provider)
			}
			val := args[1]
			var stdinData string
			callArgs := []string{"add", provider}
			if val == "stdin" {
				b, err := io.ReadAll(os.Stdin)
				if err != nil {
					return err
				}
				stdinData = string(b)
				callArgs = append(callArgs, "--stdin")
			} else if strings.HasPrefix(val, "-") {
				return fmt.Errorf("不支持短横线参数 %s，请使用: aknt key add %s stdin", val, provider)
			} else {
				callArgs = append(callArgs, val)
			}

			out, err := daemon.VaultCall(p, stdinData, callArgs...)
			if err != nil {
				return err
			}
			var res struct {
				Added int `json:"added"`
				Total int `json:"total"`
			}
			_ = json.Unmarshal(out, &res)
			s := term.Detect()
			if res.Added == 0 {
				fmt.Println(s.Warn("这些 key 已在池中"))
			} else {
				fmt.Println(s.Ok(fmt.Sprintf("%s 密钥池 %d 把（新增 %d）", provider, res.Total, res.Added)))
			}
			return nil
		},
	}

	lsCmd := &cobra.Command{
		Use:                "ls <provider>",
		Short:              "列出某 provider 的脱敏密钥池",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			out, err := daemon.VaultCall(p, "", "list", args[0])
			if err != nil {
				return err
			}
			var keys []string
			if err := json.Unmarshal(out, &keys); err != nil {
				return err
			}
			s := term.Detect()
			if len(keys) == 0 {
				fmt.Println(s.Warn(args[0] + " 的密钥池为空"))
				return nil
			}
			for i, key := range keys {
				fmt.Printf("  %d  %s\n", i+1, maskKey(key))
			}
			fmt.Println(s.Dim(fmt.Sprintf("共 %d 把", len(keys))))
			return nil
		},
	}

	rmCmd := &cobra.Command{
		Use:                "rm <provider> <序号>",
		Short:              "移除第 n 把 key",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(2),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			out, err := daemon.VaultCall(p, "", "rm", args[0], args[1])
			if err != nil {
				return err
			}
			var res struct {
				Remaining int `json:"remaining"`
			}
			_ = json.Unmarshal(out, &res)
			fmt.Println(term.Detect().Ok(fmt.Sprintf("已移除第 %s 把，剩余 %d 把", args[1], res.Remaining)))
			return nil
		},
	}

	clearCmd := &cobra.Command{
		Use:                "clear <provider>",
		Short:              "清空某 provider 的密钥池",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			if _, err := daemon.VaultCall(p, "", "clear", args[0]); err != nil {
				return err
			}
			fmt.Println(term.Detect().Ok("已清空 " + args[0] + " 的密钥池"))
			return nil
		},
	}

	sealCmd := &cobra.Command{
		Use:                "seal",
		Short:              "将明文 keys.env 迁移加密为 keys.enc",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			out, err := daemon.VaultCall(p, "", "seal")
			if err != nil {
				return err
			}
			var res struct {
				Imported int `json:"imported"`
				Entries  int `json:"entries"`
			}
			_ = json.Unmarshal(out, &res)
			fmt.Println(term.Detect().Ok(fmt.Sprintf("已加密 %d 个条目（导入明文 %d 个）", res.Entries, res.Imported)))
			return nil
		},
	}

	rekeyCmd := &cobra.Command{
		Use:                "rekey",
		Short:              "轮换主密钥或切换密钥库模式",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			out, err := daemon.VaultCall(p, "", "rekey")
			if err != nil {
				return err
			}
			var res struct {
				Mode    string `json:"mode"`
				Entries int    `json:"entries"`
			}
			_ = json.Unmarshal(out, &res)
			fmt.Println(term.Detect().Ok(fmt.Sprintf("主密钥已轮换：%s，%d 个条目", res.Mode, res.Entries)))
			return nil
		},
	}

	k.AddCommand(addCmd, lsCmd, rmCmd, clearCmd, sealCmd, rekeyCmd)
	return k
}

func cmdRouter() *cobra.Command {
	r := &cobra.Command{
		Use:                "router",
		Short:              "管理守护进程 alkanetd",
		DisableFlagParsing: true,
	}

	r.AddCommand(
		&cobra.Command{
			Use:                "start",
			Short:              "启动 alkanetd",
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				cfg, err := config.Load(p)
				if err != nil {
					return err
				}
				s := term.Detect()
				if daemon.Alive(cfg.Listen) {
					fmt.Println(s.Ok("router 已在运行 http://" + cfg.Listen))
					return nil
				}
				if err := daemon.Spawn(p); err != nil {
					return err
				}
				if !daemon.WaitAlive(cfg.Listen, 8*time.Second) {
					return fmt.Errorf("router 启动失败，看 aknt router logs（%s）", p.Log)
				}
				fmt.Println(s.Ok("router 已启动 http://" + cfg.Listen))
				return nil
			},
		},
		&cobra.Command{
			Use:                "stop",
			Short:              "停止 alkanetd",
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				cfg, err := config.Load(p)
				if err != nil {
					return err
				}
				stopped, err := daemon.Stop(p, cfg.Listen)
				if err != nil {
					return err
				}
				s := term.Detect()
				if stopped {
					fmt.Println(s.Ok("router 已停止"))
				} else {
					fmt.Println(s.Warn("router 未在运行"))
				}
				return nil
			},
		},
		&cobra.Command{
			Use:                "restart",
			Short:              "重启 alkanetd",
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				cfg, err := config.Load(p)
				if err != nil {
					return err
				}
				if _, err := daemon.Stop(p, cfg.Listen); err != nil {
					return err
				}
				if err := daemon.Spawn(p); err != nil {
					return err
				}
				if !daemon.WaitAlive(cfg.Listen, 8*time.Second) {
					return fmt.Errorf("router 启动失败，看 aknt router logs（%s）", p.Log)
				}
				fmt.Println(term.Detect().Ok("router 已重启 http://" + cfg.Listen))
				return nil
			},
		},
		&cobra.Command{
			Use:                "logs [n]",
			Short:              "查看最近 n 行日志",
			DisableFlagParsing: true,
			Args:               cobra.MaximumNArgs(1),
			RunE: func(cmd *cobra.Command, args []string) error {
				if err := rejectFlags(args); err != nil {
					return err
				}
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				n := 40
				if len(args) == 1 {
					v, err := strconv.Atoi(args[0])
					if err != nil || v <= 0 {
						return errors.New("行数必须是正整数")
					}
					n = v
				}
				txt, err := daemon.TailLog(p.Log, n)
				if err != nil {
					return err
				}
				if txt == "" {
					fmt.Println("(暂无日志)")
				} else {
					fmt.Println(txt)
				}
				return nil
			},
		},
		&cobra.Command{
			Use:                "ensure",
			Hidden:             true,
			DisableFlagParsing: true,
			Args:               cobra.NoArgs,
			RunE: func(cmd *cobra.Command, args []string) error {
				p, err := config.LoadPaths()
				if err != nil {
					return err
				}
				cfg, err := config.Load(p)
				if err != nil {
					return err
				}
				if !daemon.Alive(cfg.Listen) {
					if err := daemon.Spawn(p); err != nil {
						return err
					}
					daemon.WaitAlive(cfg.Listen, 5*time.Second)
				}
				return nil
			},
		},
	)
	return r
}

func cmdTest() *cobra.Command {
	return &cobra.Command{
		Use:                "test [router|all|<provider>]",
		Short:              "验证链路",
		DisableFlagParsing: true,
		Args:               cobra.MaximumNArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			target := "router"
			if len(args) == 1 {
				target = args[0]
			}
			return runTest(p, target)
		},
	}
}

func cmdModels() *cobra.Command {
	return &cobra.Command{
		Use:                "models <provider>",
		Short:              "拉取 provider 可用模型列表",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(1),
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			return runModels(p, args[0])
		},
	}
}

func cmdStats() *cobra.Command {
	c := &cobra.Command{
		Use:                "stats [days] [by <model|provider|tier>] [json]",
		Short:              "查看用量统计",
		DisableFlagParsing: true,
		Args:               cobra.ArbitraryArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			days := 7
			daysSet := false
			asJSON := false
			by := ""

			for i := 0; i < len(args); i++ {
				a := args[i]
				switch a {
				case "json":
					asJSON = true
				case "by":
					if i+1 >= len(args) {
						return errors.New("by 后面需要维度: model | provider | tier")
					}
					i++
					by = args[i]
					if by != "model" && by != "provider" && by != "tier" {
						return fmt.Errorf("by 只能是 model / provider / tier（收到 %s）", by)
					}
				default:
					v, err := strconv.Atoi(a)
					if err != nil || v <= 0 || daysSet {
						return fmt.Errorf("未知 stats 参数: %s（可用: aknt stats [days] [by model|provider|tier] [json] | reset）", a)
					}
					days = v
					daysSet = true
				}
			}

			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			f, err := stats.Load(p.Stats)
			if err != nil {
				return err
			}
			rep := stats.Aggregate(f, days)
			if asJSON {
				b, _ := json.MarshalIndent(rep, "", "  ")
				fmt.Println(string(b))
				return nil
			}
			fmt.Print(stats.Render(rep, term.Detect(), by))
			return nil
		},
	}
	c.AddCommand(&cobra.Command{
		Use:                "reset",
		Short:              "清空统计数据",
		DisableFlagParsing: true,
		Args:               cobra.NoArgs,
		RunE: func(cmd *cobra.Command, args []string) error {
			p, err := config.LoadPaths()
			if err != nil {
				return err
			}
			fmt.Print("确认清空全部统计? [y/N] ")
			line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
			ans := strings.ToLower(strings.TrimSpace(line))
			if ans != "y" && ans != "yes" {
				fmt.Println("已取消")
				return nil
			}
			if err := os.Remove(p.Stats); err != nil && !os.IsNotExist(err) {
				return err
			}
			fmt.Println(term.Detect().Ok("统计已清空"))
			return nil
		},
	})
	return c
}

func cmdCompletion(root *cobra.Command) *cobra.Command {
	return &cobra.Command{
		Use:                "completion <bash|zsh|fish|powershell>",
		Short:              "生成 shell 自动补全脚本",
		DisableFlagParsing: true,
		Args:               cobra.ExactArgs(1),
		ValidArgs:          []string{"bash", "zsh", "fish", "powershell"},
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := rejectFlags(args); err != nil {
				return err
			}
			switch args[0] {
			case "bash":
				return root.GenBashCompletionV2(os.Stdout, true)
			case "zsh":
				return root.GenZshCompletion(os.Stdout)
			case "fish":
				return root.GenFishCompletion(os.Stdout, true)
			case "powershell":
				return root.GenPowerShellCompletionWithDesc(os.Stdout)
			default:
				return fmt.Errorf("不支持的 shell: %s", args[0])
			}
		},
	}
}

func runDashboard(p config.Paths) error {
	cfg, err := config.Load(p)
	if err != nil {
		return err
	}
	st, err := settings.Read(p.Settings)
	if err != nil {
		return err
	}
	s := term.Detect()
	local := "http://" + cfg.Listen
	mode := "Claude"
	if bu := settings.BaseURL(st); bu == local {
		if daemon.Alive(cfg.Listen) {
			mode = "Router"
		} else {
			mode = "Router (stopped)"
		}
	} else if bu != "" {
		mode = "Claude (legacy direct)"
	}

	fmt.Printf("%s %s\n", s.Bold("Mode"), mode)
	fmt.Printf("%s %s %s\n", s.Bold("Default"), s.Tier(config.TierLabel(cfg.Default)), cfg.Effort)
	fmt.Println(s.Bold("Route List"))
	printRoutes(cfg, s)

	fmt.Println(s.Bold("API Key"))
	counts, err := fetchKeyCounts(p)
	if err != nil {
		fmt.Printf("    %s 密钥库不可用: %v\n", s.Yellow("!"), err)
	} else {
		for _, name := range sortedProviderNames(cfg.Providers) {
			cnt := counts[name]
			if cnt > 0 {
				fmt.Printf("    %s %s (%d)\n", s.Green("✓"), name, cnt)
			} else {
				fmt.Printf("    %s %s\n", s.Red("✗"), name)
			}
		}
	}
	fmt.Println(s.Dim("提示  aknt help"))
	return nil
}

func runStatus(p config.Paths, asJSON, check bool) error {
	cfg, err := config.Load(p)
	if err != nil {
		return err
	}
	st, err := settings.Read(p.Settings)
	if err != nil {
		return err
	}
	h, _ := daemon.Health(cfg.Listen)
	alive := daemon.IsHealthy(h)

	var vaultStatus map[string]any
	if out, err := daemon.VaultCall(p, "", "status"); err == nil {
		_ = json.Unmarshal(out, &vaultStatus)
	}

	claudePath, _ := exec.LookPath("claude")
	claudeVer := ""
	if claudePath != "" {
		if out, err := exec.Command(claudePath, "--version").Output(); err == nil {
			claudeVer = strings.TrimSpace(string(out))
		}
	}

	if asJSON {
		payload := map[string]any{
			"version":      Version,
			"listen":       cfg.Listen,
			"default":      cfg.Default,
			"effort":       cfg.Effort,
			"router_alive": alive,
			"health":       h,
			"routes":       cfg.Routes,
			"providers":    sortedProviderNames(cfg.Providers),
			"vault":        vaultStatus,
			"settings":     p.Settings,
			"base_url":     settings.BaseURL(st),
			"claude": map[string]any{
				"path":    claudePath,
				"version": claudeVer,
			},
		}
		b, _ := json.MarshalIndent(payload, "", "  ")
		fmt.Println(string(b))
		return nil
	}

	s := term.Detect()
	fmt.Println(s.Section("Router"))
	if alive {
		fmt.Println("  " + s.Ok("运行中 http://"+cfg.Listen))
		if up, ok := h["uptime_s"]; ok {
			fmt.Printf("  uptime   %v s\n", up)
		}
		if ver, ok := h["version"]; ok {
			fmt.Printf("  version  %v\n", ver)
		}
	} else {
		fmt.Println("  " + s.Warn("未运行 http://"+cfg.Listen+"（aknt router start）"))
	}

	fmt.Println(s.Section("密钥库"))
	if vaultStatus != nil {
		enc, _ := vaultStatus["encrypted"].(bool)
		if enc {
			fmt.Printf("  %s · %v · %v 条目\n", p.KeysEnc, vaultStatus["mode"], vaultStatus["entries"])
		} else {
			fmt.Println("  " + s.Warn("明文 keys.env（执行 aknt key seal 加密）"))
		}
	} else {
		fmt.Println("  " + s.Warn("无法读取密钥库（确认 alkanetd 可用）"))
	}

	if !check {
		return nil
	}

	fmt.Println(s.Section("环境自检"))
	fmt.Printf("  aknt     %s\n", Version)
	fmt.Printf("  配置目录 %s\n", p.Home)
	if claudePath != "" {
		fmt.Printf("  claude   %s (%s)\n", claudePath, claudeVer)
	} else {
		fmt.Println("  claude   " + s.Warn("不在 PATH"))
	}
	for _, item := range [][2]string{{"config.toml", p.Config}, {"settings.json", p.Settings}} {
		if fi, err := os.Stat(item[1]); err == nil {
			fmt.Printf("  %-14s %04o  %s\n", item[0], fi.Mode().Perm(), item[1])
		} else {
			fmt.Printf("  %-14s %s\n", item[0], s.Warn("缺失"))
		}
	}
	return nil
}

func runModels(p config.Paths, provName string) error {
	cfg, err := config.Load(p)
	if err != nil {
		return err
	}
	pr, ok := cfg.Providers[provName]
	if !ok {
		return fmt.Errorf("未知 provider: %s", provName)
	}
	keys, err := fetchProviderKeys(p, provName)
	if err != nil {
		return err
	}
	if len(keys) == 0 {
		return fmt.Errorf("%s 没有可用 key", provName)
	}
	u := strings.TrimRight(pr.BaseURL, "/") + "/v1/models"
	req, _ := http.NewRequest(http.MethodGet, u, nil)
	req.Header.Set("anthropic-version", "2023-06-01")
	if pr.UsesXAPIKey() {
		req.Header.Set("x-api-key", keys[0])
	} else {
		req.Header.Set("Authorization", "Bearer "+keys[0])
	}
	resp, err := upstreamHTTPClient(25 * time.Second).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	var parsed struct {
		Data []struct {
			ID          string `json:"id"`
			DisplayName string `json:"display_name"`
		} `json:"data"`
	}
	if json.Unmarshal(body, &parsed) == nil && len(parsed.Data) > 0 {
		for _, m := range parsed.Data {
			fmt.Printf("%s\t%s\n", m.ID, m.DisplayName)
		}
		return nil
	}
	fmt.Println("该端点没有标准 /v1/models，原始返回：")
	fmt.Println(snippet(string(body), 800))
	return nil
}

func runTest(p config.Paths, target string) error {
	cfg, err := config.Load(p)
	if err != nil {
		return err
	}
	s := term.Detect()
	switch target {
	case "router":
		if !daemon.Alive(cfg.Listen) {
			return errors.New("router 未运行（aknt router start）")
		}
		tok, err := settings.EnsureRouterToken(p)
		if err != nil {
			return err
		}
		u := "http://" + cfg.Listen + "/v1/messages"
		payload, _ := json.Marshal(map[string]any{
			"model":      "alkanet-" + cfg.Default,
			"max_tokens": 16,
			"messages":   []map[string]string{{"role": "user", "content": "say ok"}},
		})
		req, _ := http.NewRequest(http.MethodPost, u, bytes.NewReader(payload))
		req.Header.Set("anthropic-version", "2023-06-01")
		req.Header.Set("content-type", "application/json")
		req.Header.Set("x-api-key", tok)
		client := &http.Client{Timeout: 60 * time.Second, Transport: &http.Transport{Proxy: nil}}
		fmt.Println(s.Section("POST " + u))
		resp, err := client.Do(req)
		if err != nil {
			return err
		}
		defer resp.Body.Close()
		body, _ := io.ReadAll(resp.Body)
		fmt.Printf("HTTP %d\n%s\n", resp.StatusCode, snippet(string(body), 800))
		if resp.StatusCode == 200 {
			fmt.Println(s.Ok("路由链路可用"))
			return nil
		}
		return errors.New("路由链路失败，查看 aknt router logs")
	case "all":
		fails := 0
		for _, name := range sortedProviderNames(cfg.Providers) {
			if err := testProvider(p, cfg, name, s); err != nil {
				fmt.Println(s.Fail(name + ": " + err.Error()))
				fails++
			}
		}
		if fails > 0 {
			return fmt.Errorf("%d 个 provider 未通过", fails)
		}
		return nil
	default:
		return testProvider(p, cfg, target, s)
	}
}

func testProvider(p config.Paths, cfg config.AppConfig, name string, s term.Style) error {
	pr, ok := cfg.Providers[name]
	if !ok {
		return fmt.Errorf("未知 provider: %s", name)
	}
	keys, err := fetchProviderKeys(p, name)
	if err != nil {
		return err
	}
	if len(keys) == 0 {
		return fmt.Errorf("还没填 %s 的 key", name)
	}
	model := pr.EffectiveModel()
	if model == "" {
		return fmt.Errorf("%s 没有配置模型", name)
	}
	u := strings.TrimRight(pr.BaseURL, "/") + "/v1/messages"
	payload, _ := json.Marshal(map[string]any{
		"model":      model,
		"max_tokens": 16,
		"messages":   []map[string]string{{"role": "user", "content": "say ok"}},
	})
	req, _ := http.NewRequest(http.MethodPost, u, bytes.NewReader(payload))
	req.Header.Set("anthropic-version", "2023-06-01")
	req.Header.Set("content-type", "application/json")
	if pr.UsesXAPIKey() {
		req.Header.Set("x-api-key", keys[0])
	} else {
		req.Header.Set("Authorization", "Bearer "+keys[0])
	}
	fmt.Println(s.Section(fmt.Sprintf("POST %s  model=%s", u, model)))
	resp, err := upstreamHTTPClient(60 * time.Second).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	fmt.Printf("HTTP %d\n%s\n", resp.StatusCode, snippet(string(body), 800))
	if resp.StatusCode >= 200 && resp.StatusCode < 300 {
		fmt.Println(s.Ok("通道可用"))
		return nil
	}
	return errors.New("请求失败（401=key/鉴权头不对，404=model 或路径不对）")
}

func upstreamHTTPClient(timeout time.Duration) *http.Client {
	tr := &http.Transport{Proxy: nil}
	if raw := strings.TrimSpace(os.Getenv("ALKANET_PROXY")); raw != "" {
		if pu, err := url.Parse(raw); err == nil {
			tr.Proxy = http.ProxyURL(pu)
		}
	} else if raw := strings.TrimSpace(os.Getenv("CCMS_PROXY")); raw != "" {
		if pu, err := url.Parse(raw); err == nil {
			tr.Proxy = http.ProxyURL(pu)
		}
	}
	return &http.Client{Timeout: timeout, Transport: tr}
}

func fetchKeyCounts(p config.Paths) (map[string]int, error) {
	out, err := daemon.VaultCall(p, "", "counts")
	if err != nil {
		return nil, err
	}
	var m map[string]int
	if err := json.Unmarshal(out, &m); err != nil {
		return nil, err
	}
	return m, nil
}

func fetchProviderKeys(p config.Paths, name string) ([]string, error) {
	out, err := daemon.VaultCall(p, "", "list", name)
	if err != nil {
		return nil, err
	}
	var keys []string
	if err := json.Unmarshal(out, &keys); err != nil {
		return nil, err
	}
	return keys, nil
}

func printRoutes(cfg config.AppConfig, s term.Style) {
	for _, t := range config.Tiers {
		r, ok := cfg.Routes[t]
		prov, mod := "-", "-"
		if ok {
			prov, mod = r.Provider, r.Model
		}
		fmt.Printf("    %s-> %s\n", s.Tier(fmt.Sprintf("%-8s", config.TierLabel(t))), s.Cyan(prov+"/"+mod))
	}
}

func sortedProviderNames(m map[string]config.Provider) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}

func maskKey(k string) string {
	r := []rune(k)
	if len(r) <= 10 {
		return strings.Repeat("*", len(r))
	}
	return string(r[:6]) + "…" + string(r[len(r)-4:])
}

func snippet(s string, limit int) string {
	r := []rune(s)
	if len(r) <= limit {
		return s
	}
	return string(r[:limit]) + "…"
}
