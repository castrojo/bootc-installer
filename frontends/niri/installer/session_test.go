package main

// Tests for the live-session preflight (`session-get`, `session-set`) and
// the power/cancel/log-path commands. Host tools are stubbed through PATH
// the way cli_test.go stubs sudo and fisherman; pure parsers are tested
// directly.

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// localectl status shape (trimmed): the LANG value, the X11 layout first,
// the console keymap as fallback.
func TestParseLocalectlStatus(t *testing.T) {
	locale, keyboard := parseLocalectlStatus(`System Locale: LANG=en_US.UTF-8
    VC Keymap: us
   X11 Layout: de
`)
	if locale != "en_US.UTF-8" {
		t.Errorf("locale = %q", locale)
	}
	if keyboard != "de" {
		t.Errorf("keyboard = %q, want the X11 layout over the console keymap", keyboard)
	}

	locale, keyboard = parseLocalectlStatus("   System Locale: LANG=C.UTF-8\n    VC Keymap: fr\n")
	if locale != "C.UTF-8" || keyboard != "fr" {
		t.Errorf("got %q %q, want C.UTF-8 fr", locale, keyboard)
	}
}

func TestParseTimedatectlTimezoneTrims(t *testing.T) {
	if got := parseTimedatectlTimezone("Europe/Berlin\n"); got != "Europe/Berlin" {
		t.Errorf("got %q", got)
	}
}

// session-get with stubbed host tools: current values parsed, lists passed
// through, connectivity reported. The stubs stand in for localectl and
// timedatectl on the host (PATH, non-Flatpak).
func TestSessionGetReadsStubbedHostTools(t *testing.T) {
	if inFlatpak() {
		t.Skip("running inside a Flatpak sandbox")
	}
	dir := t.TempDir()
	writeStub(t, dir, "localectl", "#!/bin/sh\n"+
		"if [ \"$1\" = \"status\" ]; then echo 'System Locale: LANG=de_DE.UTF-8'; echo '   VC Keymap: de'; "+
		"elif [ \"$1\" = \"list-locales\" ]; then echo 'de_DE.UTF-8'; echo 'en_US.UTF-8'; "+
		"elif [ \"$1\" = \"list-keymaps\" ]; then echo 'de'; echo 'us'; fi\n")
	writeStub(t, dir, "timedatectl", "#!/bin/sh\n"+
		"if [ \"$1\" = \"show\" ]; then echo 'Europe/Berlin'; "+
		"elif [ \"$1\" = \"list-timezones\" ]; then echo 'Europe/Berlin'; echo 'UTC'; fi\n")
	got := runCLI(t, []string{pathWith(dir), "XDG_RUNTIME_DIR=" + t.TempDir()}, "session-get")
	if got.code != 0 {
		t.Fatalf("code = %d, stderr = %q", got.code, got.stderr)
	}
	var state SessionState
	if err := json.Unmarshal([]byte(got.stdout), &state); err != nil {
		t.Fatalf("stdout is not a SessionState: %v\n%s", err, got.stdout)
	}
	if state.Locale.Current != "de_DE.UTF-8" || len(state.Locale.Available) != 2 {
		t.Errorf("locale = %+v", state.Locale)
	}
	if state.Keyboard.Current != "de" || len(state.Keyboard.Available) != 2 {
		t.Errorf("keyboard = %+v", state.Keyboard)
	}
	if state.Timezone.Current != "Europe/Berlin" || len(state.Timezone.Available) != 2 {
		t.Errorf("timezone = %+v", state.Timezone)
	}
}

// session-set applies each requested field through the host tools and reads
// the request from stdin (never argv: values are session configuration).
func TestSessionSetAppliesEachField(t *testing.T) {
	if inFlatpak() {
		t.Skip("running inside a Flatpak sandbox")
	}
	dir := t.TempDir()
	calls := filepath.Join(dir, "calls")
	writeStub(t, dir, "localectl", "#!/bin/sh\necho \"localectl $@\" >> "+calls+"\n")
	writeStub(t, dir, "timedatectl", "#!/bin/sh\necho \"timedatectl $@\" >> "+calls+"\n")

	got := runCLIWithStdin(t, []string{pathWith(dir)}, `{"keyboard":"de","locale":"de_DE.UTF-8","timezone":"Europe/Berlin"}`)
	if got.code != 0 {
		t.Fatalf("code = %d, stderr = %q", got.code, got.stderr)
	}
	data, _ := os.ReadFile(calls)
	log := string(data)
	for _, want := range []string{"localectl set-keymap de", "localectl set-locale LANG=de_DE.UTF-8", "timedatectl set-timezone Europe/Berlin"} {
		if !strings.Contains(log, want) {
			t.Errorf("host calls lack %q:\n%s", want, log)
		}
	}
}

// A field whose tool fails is named on stderr and exits 1; garbage on
// stdin exits 1 without touching the host.
func TestSessionSetReportsAFailedField(t *testing.T) {
	if inFlatpak() {
		t.Skip("running inside a Flatpak sandbox")
	}
	dir := t.TempDir()
	writeStub(t, dir, "timedatectl", "#!/bin/sh\nexit 1\n")

	got := runCLIWithStdin(t, []string{pathWith(dir)}, `{"timezone":"Nowhere/Zero"}`)
	if got.code != 1 {
		t.Errorf("code = %d, want 1", got.code)
	}
	if !strings.Contains(got.stderr, "timezone:") {
		t.Errorf("stderr = %q, want the failed field named", got.stderr)
	}

	got = runCLIWithStdin(t, []string{pathWith(dir)}, `{nope`)
	if got.code != 1 {
		t.Errorf("code = %d, want 1 for invalid JSON", got.code)
	}
}

// cancel with no install running is success: there is nothing to stop.
func TestCancelWithoutAPidfileSucceeds(t *testing.T) {
	got := runCLI(t, []string{"XDG_RUNTIME_DIR=" + t.TempDir()}, "cancel")
	if got.code != 0 {
		t.Errorf("code = %d, stderr = %q; want 0 with nothing to cancel", got.code, got.stderr)
	}
}

// cancel kills the recorded process group: a sleeper started in its own
// group (the shape wrapperCommand gives the install wrapper) must be gone
// after `cancel`, and the pid file with it.
func TestCancelKillsTheRecordedProcessGroup(t *testing.T) {
	runtime := t.TempDir()
	started := make(chan int, 1)
	go func() {
		cmd := testSleeper(t, runtime)
		started <- cmd.Process.Pid
		_ = cmd.Wait()
	}()
	pid := <-started
	writePid := func() {
		if err := os.WriteFile(filepath.Join(runtime, installPidName), []byte(itoa(pid)), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	writePid()

	got := runCLI(t, []string{"XDG_RUNTIME_DIR=" + runtime}, "cancel")
	if got.code != 0 {
		t.Fatalf("code = %d, stderr = %q", got.code, got.stderr)
	}
	deadline := time.Now().Add(10 * time.Second)
	for processGroupAlive(pid) {
		if time.Now().After(deadline) {
			t.Fatalf("process group %d still alive after cancel", pid)
		}
		time.Sleep(100 * time.Millisecond)
	}
	if _, err := os.Stat(filepath.Join(runtime, installPidName)); !os.IsNotExist(err) {
		t.Errorf("pid file survived the cancel")
	}
}

// log-path prints the same file the installer appends to.
func TestLogPathNamesTheInstallLog(t *testing.T) {
	stateHome := t.TempDir()
	got := runCLI(t, []string{"XDG_STATE_HOME=" + stateHome}, "log-path")
	if got.code != 0 {
		t.Fatalf("code = %d, stderr = %q", got.code, got.stderr)
	}
	want := filepath.Join(stateHome, "tuna-installer", "install.log")
	if strings.TrimSpace(got.stdout) != want {
		t.Errorf("log-path = %q, want %q", got.stdout, want)
	}
}

// poweroff and reboot shell to systemctl on the host: the stub records the
// call, and a failing systemctl exits 1 with the reason on stderr.
func TestPowerCommandsReachHostSystemctl(t *testing.T) {
	if inFlatpak() {
		t.Skip("running inside a Flatpak sandbox")
	}
	for _, tc := range []struct {
		cmd  string
		want string
	}{
		{"poweroff", "poweroff"},
		{"reboot", "reboot"},
		{"firmware", "reboot --firmware-setup"},
	} {
		dir := t.TempDir()
		calls := filepath.Join(dir, "calls")
		writeStub(t, dir, "systemctl", "#!/bin/sh\necho \"$@\" > "+calls+"\n")
		got := runCLI(t, []string{pathWith(dir)}, tc.cmd)
		if got.code != 0 {
			t.Fatalf("%s: code = %d, stderr = %q", tc.cmd, got.code, got.stderr)
		}
		data, _ := os.ReadFile(calls)
		if strings.TrimSpace(string(data)) != tc.want {
			t.Errorf("%s ran systemctl %q, want %q", tc.cmd, data, tc.want)
		}
	}
}
