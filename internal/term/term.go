package term

import (
	"fmt"
	"os"
	"strings"
)

type Style struct {
	color  bool
	accent string
}

func Detect() Style {
	fi, err := os.Stdout.Stat()
	isTTY := err == nil && (fi.Mode()&os.ModeCharDevice) != 0
	if !isTTY || os.Getenv("NO_COLOR") != "" {
		return Style{}
	}
	ct := os.Getenv("COLORTERM")
	t := os.Getenv("TERM")
	accent := "\x1b[33m"
	if strings.Contains(ct, "truecolor") || strings.Contains(ct, "24bit") {
		accent = "\x1b[38;2;217;119;87m"
	} else if strings.Contains(t, "256color") {
		accent = "\x1b[38;5;173m"
	}
	return Style{color: true, accent: accent}
}

func (s Style) wrap(code, text string) string {
	if !s.color || code == "" {
		return text
	}
	return code + text + "\x1b[0m"
}

func (s Style) Bold(t string) string    { return s.wrap("\x1b[1m", t) }
func (s Style) Dim(t string) string     { return s.wrap("\x1b[2m", t) }
func (s Style) Cyan(t string) string    { return s.wrap("\x1b[36m", t) }
func (s Style) Green(t string) string   { return s.wrap("\x1b[32m", t) }
func (s Style) Red(t string) string     { return s.wrap("\x1b[31m", t) }
func (s Style) Yellow(t string) string  { return s.wrap("\x1b[33m", t) }
func (s Style) Tier(t string) string    { return s.wrap(s.accent, t) }
func (s Style) Section(t string) string { return s.Cyan(fmt.Sprintf("== %s ==", t)) }
func (s Style) Ok(t string) string      { return fmt.Sprintf("%s %s", s.Green("✓"), t) }
func (s Style) Warn(t string) string    { return fmt.Sprintf("%s %s", s.Yellow("!"), t) }
func (s Style) Fail(t string) string    { return fmt.Sprintf("%s %s", s.Red("✗"), t) }
