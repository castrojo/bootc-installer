package main

// Live-session preflight (GNOME defaults/* parity, applied to the live
// session only).
//
// The fisherman recipe has no language, keyboard, timezone or network keys —
// shared/recipe/fisherman-recipe.schema.json carries none, and PARITY.md
// keeps locale steps GNOME-only by design. What this backend offers instead
// is the part that works without recipe support: reading and applying the
// LIVE session's keyboard layout, locale and timezone (localectl and
// timedatectl on the host, the same tools every other frontend shells out
// to), and a connectivity probe for the download the Source step may need
// (GNOME's conn_check recheck button).
//
// `session-get` reports the current values plus what can be chosen and
// whether the network answers; `session-set` applies one JSON object from
// stdin ({keyboard, locale, timezone}, each optional). Both are best-effort
// reads and privileged writes: a check that cannot run reports "", and a
// write that fails names the field on stderr and exits 1.

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"strings"
	"time"
)

// sessionTimeout bounds one host query. Listing timezones shells out, so this
// bounds a hung flatpak-spawn, not computation.
const sessionTimeout = 10 * time.Second

// netcheckTarget is the connectivity probe: a TCP handshake with a public
// DNS resolver, which downloads nothing and answers whether an image pull
// can work. GNOME's conn_check dials out the same way.
const netcheckTarget = "1.1.1.1:53"

// netcheckTimeout bounds the handshake; the Source step must not wait on it.
const netcheckTimeout = 5 * time.Second

// SessionState is what the QML Session step renders and edits.
type SessionState struct {
	Keyboard  SessionChoice `json:"keyboard"`
	Locale    SessionChoice `json:"locale"`
	Timezone  SessionChoice `json:"timezone"`
	Connected bool          `json:"connected"`
}

// SessionChoice is one setting: what the session uses now, and what it may
// be changed to. Available is empty when the listing tool is missing.
type SessionChoice struct {
	Current   string   `json:"current"`
	Available []string `json:"available"`
}

func emptyChoice() SessionChoice {
	return SessionChoice{Available: []string{}}
}

// runHostQuery runs argv on the host with a timeout and returns stdout. A
// failure yields "" rather than an error: every session fact is optional,
// and the QML renders what it got.
func runHostQuery(timeout time.Duration, argv ...string) string {
	ctx, cancel := context.WithTimeout(context.Background(), timeout)
	defer cancel()
	cmd := exec.CommandContext(ctx, argv[0], argv[1:]...)
	var out bytes.Buffer
	cmd.Stdout = &out
	if err := cmd.Run(); err != nil {
		return ""
	}
	return out.String()
}

// nonEmptyLines splits command output into its non-blank lines.
func nonEmptyLines(out string) []string {
	lines := []string{}
	for _, line := range strings.Split(out, "\n") {
		if trimmed := strings.TrimSpace(line); trimmed != "" {
			lines = append(lines, trimmed)
		}
	}
	return lines
}

// parseLocalectlStatus reads `localectl status`: the System Locale's LANG
// value, and the X11 layout (falling back to the console keymap).
func parseLocalectlStatus(out string) (locale, keyboard string) {
	for _, line := range strings.Split(out, "\n") {
		name, value, ok := strings.Cut(line, ":")
		if !ok {
			continue
		}
		switch strings.TrimSpace(name) {
		case "System Locale":
			for _, field := range strings.Fields(value) {
				if lang, found := strings.CutPrefix(strings.TrimSpace(field), "LANG="); found {
					locale = lang
				}
			}
		case "X11 Layout":
			keyboard = strings.TrimSpace(value)
		case "VC Keymap":
			if keyboard == "" {
				keyboard = strings.TrimSpace(value)
			}
		}
	}
	return locale, keyboard
}

// parseTimedatectlTimezone reads `timedatectl show -p Timezone --value`.
func parseTimedatectlTimezone(out string) string {
	return strings.TrimSpace(out)
}

// checkConnectivity dials the probe target: connected reports the handshake,
// and latencyMs how long it took (-1 when it failed).
func checkConnectivity() (connected bool, latencyMs int64) {
	start := time.Now()
	conn, err := net.DialTimeout("tcp", netcheckTarget, netcheckTimeout)
	if err != nil {
		return false, -1
	}
	_ = conn.Close()
	return true, time.Since(start).Milliseconds()
}

// sessionGet gathers the live session's state. The list commands come from
// the host tools every installer already shells out to; each is optional.
func sessionGet() SessionState {
	state := SessionState{
		Keyboard: emptyChoice(), Locale: emptyChoice(), Timezone: emptyChoice(),
	}
	host := hostCommand()
	run := func(args ...string) string {
		return runHostQuery(sessionTimeout, append(append([]string{}, host...), args...)...)
	}
	locale, keyboard := parseLocalectlStatus(run("localectl", "status"))
	state.Locale.Current = locale
	state.Locale.Available = nonEmptyLines(run("localectl", "list-locales"))
	state.Keyboard.Current = keyboard
	state.Keyboard.Available = nonEmptyLines(run("localectl", "list-keymaps"))
	state.Timezone.Current = parseTimedatectlTimezone(run("timedatectl", "show", "-p", "Timezone", "--value"))
	state.Timezone.Available = nonEmptyLines(run("timedatectl", "list-timezones"))
	state.Connected, _ = checkConnectivity()
	return state
}

// sessionApply carries one `session-set` request: every field is optional,
// and only the fields present are applied.
type sessionApply struct {
	Keyboard string `json:"keyboard"`
	Locale   string `json:"locale"`
	Timezone string `json:"timezone"`
}

// applySession writes the requested settings to the live session through
// the host tools. setxkbmap covers Xwayland where it exists; localectl
// owns the console and the requested default. A field that fails is
// returned as an error and the rest still apply.
func applySession(want sessionApply) map[string]string {
	failures := map[string]string{}
	host := hostCommand()
	run := func(args ...string) error {
		argv := append(append([]string{}, host...), args...)
		ctx, cancel := context.WithTimeout(context.Background(), sessionTimeout)
		defer cancel()
		return exec.CommandContext(ctx, argv[0], argv[1:]...).Run()
	}
	if want.Keyboard != "" {
		if err := run("localectl", "set-keymap", want.Keyboard); err != nil {
			failures["keyboard"] = err.Error()
		} else if out := runHostQuery(sessionTimeout, append(host, "which", "setxkbmap")...); out != "" {
			// Best-effort: no X11 session means nothing to set.
			_ = run("setxkbmap", want.Keyboard)
		}
	}
	if want.Locale != "" {
		if err := run("localectl", "set-locale", "LANG="+want.Locale); err != nil {
			failures["locale"] = err.Error()
		}
	}
	if want.Timezone != "" {
		if err := run("timedatectl", "set-timezone", want.Timezone); err != nil {
			failures["timezone"] = err.Error()
		}
	}
	return failures
}

func sessionGetMain() {
	state := sessionGet()
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(state); err != nil {
		fmt.Fprintf(os.Stderr, "encode output: %v\n", err)
		os.Exit(1)
	}
}

func sessionSetMain() {
	data, err := os.ReadFile("/dev/stdin")
	if err != nil {
		fmt.Fprintf(os.Stderr, "read settings: %v\n", err)
		os.Exit(1)
	}
	var want sessionApply
	if err := json.Unmarshal(data, &want); err != nil {
		fmt.Fprintf(os.Stderr, "settings are not valid JSON: %v\n", err)
		os.Exit(1)
	}
	if failures := applySession(want); len(failures) > 0 {
		for field, reason := range failures {
			fmt.Fprintf(os.Stderr, "%s: %s\n", field, reason)
		}
		os.Exit(1)
	}
}
