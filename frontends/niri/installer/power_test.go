package main

// Helpers for the session/power tests, plus the pid-file contract between
// runInstall and `cancel`.

import (
	"bytes"
	"encoding/json"
	"os"
	"os/exec"
	"strconv"
	"strings"
	"syscall"
	"testing"
)

// runCLIWithStdin is runCLI with bytes on the child's stdin (session-set
// reads its JSON there, the way install reads its recipe there).
func runCLIWithStdin(t *testing.T, env []string, stdin string) cliResult {
	t.Helper()
	cmd := exec.Command(os.Args[0])
	cmd.Env = append(os.Environ(), "GO_TEST_CLI_ARGV=session-set")
	cmd.Env = append(cmd.Env, env...)
	cmd.Stdin = strings.NewReader(stdin)
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	code := 0
	if err := cmd.Run(); err != nil {
		if exitErr, ok := err.(*exec.ExitError); ok {
			code = exitErr.ExitCode()
		} else {
			t.Fatalf("running the backend: %v", err)
		}
	}
	return cliResult{stdout: stdout.String(), stderr: stderr.String(), code: code}
}

func itoa(n int) string { return strconv.Itoa(n) }

// testSleeper starts a `sleep` in its own process group — the shape
// wrapperCommand gives the install wrapper, which is what `cancel`
// addresses with kill(-pgid).
func testSleeper(t *testing.T, dir string) *exec.Cmd {
	t.Helper()
	cmd := exec.Command("sleep", "60")
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	cmd.Env = append(os.Environ(), "TMPDIR="+dir)
	if err := cmd.Start(); err != nil {
		t.Fatalf("starting sleeper: %v", err)
	}
	t.Cleanup(func() { _ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL) })
	return cmd
}

func processGroupAlive(pid int) bool {
	return syscall.Kill(-pid, 0) == nil
}

// The pid file records exactly the wrapper's pid as decimal text, so
// `cancel` addresses the group `kill` expects.
func TestPidfileRoundTrip(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", dir)
	writePidfile(4242)
	if got := readPidfile(); got != 4242 {
		t.Errorf("readPidfile = %d, want 4242", got)
	}
	clearPidfile()
	if got := readPidfile(); got != 0 {
		t.Errorf("readPidfile after clear = %d, want 0", got)
	}
}

// Garbage in the pid file is "nothing to cancel", not a kill of pid 0
// (which would signal the whole process group).
func TestPidfileGarbageIsNothingToCancel(t *testing.T) {
	dir := t.TempDir()
	t.Setenv("XDG_RUNTIME_DIR", dir)
	if err := os.WriteFile(dir+"/"+installPidName, []byte("not-a-pid"), 0o644); err != nil {
		t.Fatal(err)
	}
	if got := readPidfile(); got != 0 {
		t.Errorf("readPidfile = %d for garbage, want 0", got)
	}
	cancelInstall() // must return, and must not signal anyone
}

// detect's logPath is the file log-path prints: one source of truth for the
// path the QML names next to its Copy button.
func TestDetectLogPathMatchesLogPathCommand(t *testing.T) {
	stateHome := t.TempDir()
	env := []string{"XDG_STATE_HOME=" + stateHome, fakeProbeEnv + "=/nonexistent/probe.json"}
	detect := runCLI(t, env, "detect")
	logpath := runCLI(t, []string{"XDG_STATE_HOME=" + stateHome}, "log-path")
	if detect.code != 0 || logpath.code != 0 {
		t.Fatalf("codes = %d, %d", detect.code, logpath.code)
	}
	var doc struct {
		LogPath string `json:"logPath"`
	}
	if err := json.Unmarshal([]byte(detect.stdout), &doc); err != nil {
		t.Fatalf("detect stdout: %v", err)
	}
	if doc.LogPath != strings.TrimSpace(logpath.stdout) {
		t.Errorf("detect logPath = %q, log-path = %q", doc.LogPath, logpath.stdout)
	}
}
