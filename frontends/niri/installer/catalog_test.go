package main

// Tests for the `catalog` subcommand (XFCE SourcePage parity): inheritance
// across group->leaf nodes, registry+tag resolution, and the graceful empty
// catalog the QML falls back from.

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func writeCatalog(t *testing.T, body string) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), "images.json")
	if err := os.WriteFile(path, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func runCatalog(t *testing.T, path string) (map[string]any, string, int) {
	t.Helper()
	got := runCLI(t, []string{imagesPathEnv + "=" + path}, "catalog")
	var doc map[string]any
	if got.code == 0 {
		if err := json.Unmarshal([]byte(got.stdout), &doc); err != nil {
			t.Fatalf("catalog stdout is not JSON: %v\n%s", err, got.stdout)
		}
	}
	return doc, got.stderr, got.code
}

// Leaves inherit the group's registry, filesystem, bootloader, flatpaks and
// needs_user_creation; a leaf overrides what it names. Mirrors XFCE
// ImageNode's root->leaf resolution.
func TestCatalogResolvesInheritance(t *testing.T) {
	path := writeCatalog(t, `{
		"default_image": "ghcr.io/example/os:stable",
		"images": [{
			"name": "Example OS", "registry": "ghcr.io/example",
			"filesystem": "btrfs", "bootloader": "systemd",
			"flatpaks": "org.gnome.Calculator org.gnome.TextEditor",
			"children": [
				{"name": "Stable", "tag": "stable"},
				{"name": "Beta", "tag": "beta", "filesystem": "ext4",
				 "needs_user_creation": false, "flatpaks": ["org.gnome.Maps"]}
			]
		}]
	}`)
	doc, stderr, code := runCatalog(t, path)
	if code != 0 {
		t.Fatalf("code = %d, stderr = %q", code, stderr)
	}
	if doc["default_image"] != "ghcr.io/example/os:stable" {
		t.Errorf("default_image = %v", doc["default_image"])
	}
	leaves, ok := doc["leaves"].([]any)
	if !ok || len(leaves) != 2 {
		t.Fatalf("leaves = %v, want two", doc["leaves"])
	}
	stable := leaves[0].(map[string]any)
	if stable["imgref"] != "ghcr.io/example:stable" {
		t.Errorf("stable imgref = %v, want registry:tag", stable["imgref"])
	}
	for _, key := range []string{"filesystem", "bootloader"} {
		_ = key
	}
	if stable["filesystem"] != "btrfs" || stable["bootloader"] != "systemd" {
		t.Errorf("stable inherited %v", stable)
	}
	if flats, _ := stable["flatpaks"].([]any); len(flats) != 2 {
		t.Errorf("stable flatpaks = %v, want the manifest string split", stable["flatpaks"])
	}
	if stable["needs_user_creation"] != true {
		t.Errorf("stable needs_user_creation = %v, want the true default", stable["needs_user_creation"])
	}
	beta := leaves[1].(map[string]any)
	if beta["filesystem"] != "ext4" {
		t.Errorf("beta filesystem = %v, want the leaf override", beta["filesystem"])
	}
	if beta["needs_user_creation"] != false {
		t.Errorf("beta needs_user_creation = %v, want false", beta["needs_user_creation"])
	}
	if flats, _ := beta["flatpaks"].([]any); len(flats) != 1 || flats[0] != "org.gnome.Maps" {
		t.Errorf("beta flatpaks = %v, want the leaf list", beta["flatpaks"])
	}
}

// A "@"-prefixed flatpaks value is a catalog reference, not a package list
// (XFCE build_recipe drops it); the leaf reports no flatpaks.
func TestCatalogDropsCatalogReferenceFlatpaks(t *testing.T) {
	path := writeCatalog(t, `{"images": [
		{"name": "A", "imgref": "ghcr.io/example/a:1", "flatpaks": "@desktop"}
	]}`)
	doc, _, code := runCatalog(t, path)
	if code != 0 {
		t.Fatalf("code = %d", code)
	}
	leaf := doc["leaves"].([]any)[0].(map[string]any)
	if flats, _ := leaf["flatpaks"].([]any); len(flats) != 0 {
		t.Errorf("flatpaks = %v, want empty for a @-reference", leaf["flatpaks"])
	}
}

// Carousel slides pass through with title, description and image; junk
// entries are dropped so the QML tour never renders an empty slide.
func TestCatalogPassesCarouselSlidesThrough(t *testing.T) {
	path := writeCatalog(t, `{"images": [
		{"name": "A", "imgref": "ghcr.io/example/a:1",
		 "carousel": [{"title": "Welcome", "description": "Hi", "image": "/a.png"}, {}, 42]}
	]}`)
	doc, _, code := runCatalog(t, path)
	if code != 0 {
		t.Fatalf("code = %d", code)
	}
	leaf := doc["leaves"].([]any)[0].(map[string]any)
	slides, _ := leaf["carousel"].([]any)
	if len(slides) != 1 {
		t.Fatalf("carousel = %v, want the one real slide", leaf["carousel"])
	}
	slide := slides[0].(map[string]any)
	if slide["title"] != "Welcome" || slide["image"] != "/a.png" {
		t.Errorf("slide = %v", slide)
	}
}

// No catalog file is the live-ISO norm, not an error: empty leaves, exit 0,
// and the QML falls back to the branding default image.
func TestCatalogWithoutAFileIsEmpty(t *testing.T) {
	doc, _, code := runCatalog(t, "/nonexistent/images.json")
	if code != 0 {
		t.Fatalf("code = %d, want 0 with no catalog file", code)
	}
	if leaves, _ := doc["leaves"].([]any); len(leaves) != 0 {
		t.Errorf("leaves = %v, want none", doc["leaves"])
	}
}

// A corrupt manifest fails loudly on stderr, with nothing JSON on stdout
// for the QML to misread as an empty catalog.
func TestCatalogRejectsInvalidJSON(t *testing.T) {
	path := writeCatalog(t, `{not json`)
	got := runCLI(t, []string{imagesPathEnv + "=" + path}, "catalog")
	if got.code != 1 {
		t.Errorf("code = %d, want 1", got.code)
	}
	if !strings.Contains(got.stderr, "images.json") {
		t.Errorf("stderr = %q, want the reason", got.stderr)
	}
	if got.stdout != "" {
		t.Errorf("stdout = %q, want empty", got.stdout)
	}
}
