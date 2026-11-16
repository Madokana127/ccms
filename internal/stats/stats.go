package stats

import (
	"encoding/json"
	"fmt"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/Madokana127/alkanet/internal/term"
)

type Bucket struct {
	Requests                 uint64 `json:"requests"`
	InputTokens              uint64 `json:"input_tokens"`
	OutputTokens             uint64 `json:"output_tokens"`
	CacheReadInputTokens     uint64 `json:"cache_read_input_tokens"`
	CacheCreationInputTokens uint64 `json:"cache_creation_input_tokens"`
	Retries                  uint64 `json:"retries"`
	Errors                   uint64 `json:"errors"`
	DurationMs               uint64 `json:"duration_ms"`
}

func (b *Bucket) add(o Bucket) {
	b.Requests += o.Requests
	b.InputTokens += o.InputTokens
	b.OutputTokens += o.OutputTokens
	b.CacheReadInputTokens += o.CacheReadInputTokens
	b.CacheCreationInputTokens += o.CacheCreationInputTokens
	b.Retries += o.Retries
	b.Errors += o.Errors
	b.DurationMs += o.DurationMs
}

func (b Bucket) AvgMs() uint64 {
	if b.Requests == 0 {
		return 0
	}
	return b.DurationMs / b.Requests
}

type Day struct {
	Total     Bucket            `json:"total"`
	Providers map[string]Bucket `json:"providers"`
	Tiers     map[string]Bucket `json:"tiers"`
	Models    map[string]Bucket `json:"models"`
}

type File struct {
	Version uint32         `json:"version"`
	Daily   map[string]Day `json:"daily"`
}

type Row struct {
	Name   string `json:"name"`
	Bucket Bucket `json:"bucket"`
}

type Report struct {
	Today      string `json:"today"`
	Days       int    `json:"days"`
	From       string `json:"from"`
	To         string `json:"to"`
	Total      Bucket `json:"total"`
	TodayTotal Bucket `json:"today_total"`
	ByModel    []Row  `json:"by_model"`
	ByProvider []Row  `json:"by_provider"`
	ByTier     []Row  `json:"by_tier"`
}

func Load(path string) (File, error) {
	b, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			return File{Version: 2, Daily: map[string]Day{}}, nil
		}
		return File{}, fmt.Errorf("读取统计文件 %s: %w", path, err)
	}
	var f File
	if err := json.Unmarshal(b, &f); err != nil {
		return File{}, fmt.Errorf("%s 已损坏或不是合法统计 JSON: %w", path, err)
	}
	if f.Daily == nil {
		f.Daily = map[string]Day{}
	}
	return f, nil
}

func Aggregate(f File, days int) Report {
	if days < 1 {
		days = 1
	}
	now := time.Now()
	today := now.Format("2006-01-02")
	from := now.AddDate(0, 0, -(days - 1)).Format("2006-01-02")

	rep := Report{
		Today: today,
		Days:  days,
		From:  from,
		To:    today,
	}

	models := map[string]Bucket{}
	providers := map[string]Bucket{}
	tiers := map[string]Bucket{}

	for date, day := range f.Daily {
		if date < from || date > today {
			continue
		}
		rep.Total.add(day.Total)
		merge(models, day.Models)
		merge(providers, day.Providers)
		merge(tiers, day.Tiers)
	}
	if d, ok := f.Daily[today]; ok {
		rep.TodayTotal = d.Total
	}
	rep.ByModel = sortRows(models)
	rep.ByProvider = sortRows(providers)
	rep.ByTier = sortRows(tiers)
	return rep
}

func merge(dst, src map[string]Bucket) {
	for k, v := range src {
		b := dst[k]
		b.add(v)
		dst[k] = b
	}
}

func sortRows(m map[string]Bucket) []Row {
	rows := make([]Row, 0, len(m))
	for k, v := range m {
		rows = append(rows, Row{Name: k, Bucket: v})
	}
	sort.Slice(rows, func(i, j int) bool {
		if rows[i].Bucket.Requests != rows[j].Bucket.Requests {
			return rows[i].Bucket.Requests > rows[j].Bucket.Requests
		}
		return rows[i].Name < rows[j].Name
	})
	return rows
}

func Render(r Report, s term.Style, only string) string {
	if r.Total.Requests == 0 {
		return "暂无用量记录（启用路由并产生请求后自动统计）\n"
	}
	var out strings.Builder
	show := func(dim string) bool {
		return only == "" || only == dim
	}
	if only == "" {
		out.WriteString(renderBlock("Today  "+r.Today, r.TodayTotal, s))
		out.WriteByte('\n')
		out.WriteString(renderBlock(fmt.Sprintf("Last %d days  (%s ~ %s)", r.Days, r.From, r.To), r.Total, s))
	}
	if show("model") {
		out.WriteString(renderTable("By model", "MODEL", r.ByModel, s))
	}
	if show("provider") {
		out.WriteString(renderTable("By provider", "PROVIDER", r.ByProvider, s))
	}
	if show("tier") {
		out.WriteString(renderTable("By tier", "TIER", r.ByTier, s))
	}
	return out.String()
}

func renderBlock(title string, b Bucket, s term.Style) string {
	var out strings.Builder
	out.WriteString(s.Bold(title) + "\n")
	fmt.Fprintf(&out, "  %-10s%14s\n", "requests", fmtNum(b.Requests))
	fmt.Fprintf(&out, "  %-10s%14s\n", "input", fmtNum(b.InputTokens))
	fmt.Fprintf(&out, "  %-10s%14s\n", "output", fmtNum(b.OutputTokens))
	if b.CacheReadInputTokens > 0 || b.CacheCreationInputTokens > 0 {
		fmt.Fprintf(&out, "  %-10s%14s\n", "cache r/w", fmtNum(b.CacheReadInputTokens)+" / "+fmtNum(b.CacheCreationInputTokens))
	}
	if b.Retries > 0 || b.Errors > 0 {
		fmt.Fprintf(&out, "  %-10s%14s\n", "retries", fmt.Sprintf("%s (err %s)", fmtNum(b.Retries), fmtNum(b.Errors)))
	}
	fmt.Fprintf(&out, "  %-10s%14s\n", "avg ms", fmtNum(b.AvgMs()))
	return out.String()
}

func renderTable(title, head string, rows []Row, s term.Style) string {
	if len(rows) == 0 {
		return ""
	}
	var out strings.Builder
	out.WriteString("\n" + s.Bold(title) + "\n")
	fmt.Fprintf(&out, "  %-30s%7s%14s%12s%9s\n", head, "REQ", "INPUT", "OUTPUT", "AVG MS")
	for _, r := range rows {
		fmt.Fprintf(&out, "  %-30s%7s%14s%12s%9s\n",
			r.Name, fmtNum(r.Bucket.Requests), fmtNum(r.Bucket.InputTokens), fmtNum(r.Bucket.OutputTokens), fmtNum(r.Bucket.AvgMs()))
	}
	return out.String()
}

func fmtNum(v uint64) string {
	s := fmt.Sprintf("%d", v)
	if len(s) <= 3 {
		return s
	}
	var out strings.Builder
	rem := len(s) % 3
	if rem > 0 {
		out.WriteString(s[:rem])
		if len(s) > rem {
			out.WriteByte(',')
		}
	}
	for i := rem; i < len(s); i += 3 {
		out.WriteString(s[i : i+3])
		if i+3 < len(s) {
			out.WriteByte(',')
		}
	}
	return out.String()
}
