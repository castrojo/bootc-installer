package main

// Image catalog for the Source step (XFCE SourcePage parity).
//
// fisherman ships an images.json manifest (its data/images.json, overridable
// per distro); the GNOME and XFCE frontends offer one leaf per row and take
// the image ref, the filesystem default, the bootloader, composefs, flatpaks
// and needs_user_creation from the selected leaf. This backend reads the
// same files so the QML can offer the same choice: `catalog` prints the flat
// leaf list with root->leaf inheritance resolved, exactly the way
// frontends/xfce/tuna_installer_xfce/core.py's ImageNode does (registry+tag
// or imgref, needs_user_creation defaulting to true).
//
// The offline mark is NOT resolved here: whether a leaf needs a download is
// the probe's offline.images fact (detect), and the QML joins the two, so
// this command stays runnable with no probe and no network.

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// imagesPathEnv overrides the catalog path outright, for tests and for
// distros iterating on a manifest without touching system paths.
const imagesPathEnv = "FISHERMAN_IMAGES_PATH"

// catalogPaths mirrors XFCE's IMAGES_JSON_PATHS (plus the GNOME system path,
// which names this installer rather than fisherman): per-run override,
// per-user config, system override, bundled data.
func catalogPaths() []string {
	paths := []string{}
	if p := os.Getenv(imagesPathEnv); p != "" {
		paths = append(paths, p)
	}
	if cfg, err := os.UserConfigDir(); err == nil {
		paths = append(paths, filepath.Join(cfg, "tuna-installer", "images.json"))
	}
	return append(paths,
		"/etc/tuna-installer/images.json",
		"/etc/bootc-installer/images.json",
		"/app/share/fisherman/data/images.json",
		"/usr/share/fisherman/data/images.json",
	)
}

// CatalogLeaf is one installable image: a leaf node with every inherited
// field resolved. Flatpaks is always a list (a manifest string is split on
// whitespace, mirroring XFCE's build_recipe); a "@"-prefixed value is a
// catalog reference, not a package list, and resolves to empty here, with
// the QML leaving the recipe key out the same way.
type CatalogLeaf struct {
	Name              string          `json:"name"`
	Desc              string          `json:"desc"`
	Imgref            string          `json:"imgref"`
	Filesystem        string          `json:"filesystem"`
	Filesystems       []string        `json:"filesystems"`
	NeedsUserCreation bool            `json:"needs_user_creation"`
	Bootloader        string          `json:"bootloader"`
	Composefs         bool            `json:"composefs"`
	Flatpaks          []string        `json:"flatpaks"`
	DefaultHostname   string          `json:"default_hostname"`
	Carousel          []CarouselSlide `json:"carousel"`
}

// CarouselSlide is one tour slide from a leaf's "carousel" list. Keys beyond
// these three are ignored; a slide the QML cannot render (no title and no
// description and no image) is dropped.
type CarouselSlide struct {
	Title       string `json:"title"`
	Description string `json:"description"`
	Image       string `json:"image"`
}

// loadCatalog reads the first catalog file that exists and parses. An empty
// catalog (no file, or a file with no leaves) is not an error: the QML falls
// back to the branding default image and says the catalog is missing, the
// way XFCE's SourcePage warns instead of failing.
func loadCatalog() (defaultImage string, leaves []CatalogLeaf, err error) {
	for _, path := range catalogPaths() {
		if path == "" {
			continue
		}
		data, readErr := os.ReadFile(path)
		if readErr != nil {
			continue
		}
		return parseCatalog(data)
	}
	return "", []CatalogLeaf{}, nil
}

func parseCatalog(data []byte) (string, []CatalogLeaf, error) {
	var doc struct {
		DefaultImage string           `json:"default_image"`
		Images       []catalogRawNode `json:"images"`
	}
	if err := json.Unmarshal(data, &doc); err != nil {
		return "", nil, fmt.Errorf("images.json is not valid JSON: %w", err)
	}
	leaves := []CatalogLeaf{}
	for _, node := range doc.Images {
		leaves = append(leaves, node.leaves(nil)...)
	}
	return doc.DefaultImage, leaves, nil
}

// catalogRawNode is one manifest node before inheritance is resolved. Every
// scalar inherits from its parent; needs_user_creation defaults to true at
// the root, mirroring XFCE's ImageNode.
type catalogRawNode struct {
	Name            string            `json:"name"`
	Desc            string            `json:"desc"`
	Registry        string            `json:"registry"`
	Tag             string            `json:"tag"`
	Imgref          string            `json:"imgref"`
	Filesystem      string            `json:"filesystem"`
	Filesystems     []string          `json:"filesystems"`
	NeedsUser       *bool             `json:"needs_user_creation"`
	Bootloader      string            `json:"bootloader"`
	Composefs       *bool             `json:"composefs"`
	Flatpaks        any               `json:"flatpaks"`
	DefaultHostname string            `json:"default_hostname"`
	Carousel        []json.RawMessage `json:"carousel"`
	Children        []catalogRawNode  `json:"children"`
}

func (n catalogRawNode) imgrefOf(parent *catalogRawNode) string {
	if n.Imgref != "" {
		return n.Imgref
	}
	registry := n.Registry
	if registry == "" && parent != nil {
		registry = parent.Registry
	}
	if registry != "" && n.Tag != "" {
		return registry + ":" + n.Tag
	}
	return ""
}

func (n catalogRawNode) flatpaksOf(parent *catalogRawNode) []string {
	raw := n.Flatpaks
	if raw == nil && parent != nil {
		raw = parent.Flatpaks
	}
	switch v := raw.(type) {
	case []any:
		out := []string{}
		for _, item := range v {
			if s, ok := item.(string); ok && s != "" {
				out = append(out, s)
			}
		}
		return out
	case string:
		if strings.HasPrefix(v, "@") || v == "" {
			return []string{}
		}
		return strings.Fields(v)
	default:
		return []string{}
	}
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

// leaves resolves this node: a node with an imgref and no children is a
// selectable image; anything else only contributes inherited fields.
func (n catalogRawNode) leaves(parent *catalogRawNode) []CatalogLeaf {
	merged := n
	if parent != nil {
		if merged.Registry == "" {
			merged.Registry = parent.Registry
		}
		if merged.Filesystem == "" {
			merged.Filesystem = parent.Filesystem
		}
		if len(merged.Filesystems) == 0 {
			merged.Filesystems = parent.Filesystems
		}
		if merged.NeedsUser == nil {
			merged.NeedsUser = parent.NeedsUser
		}
		if merged.Bootloader == "" {
			merged.Bootloader = parent.Bootloader
		}
		if merged.Composefs == nil {
			merged.Composefs = parent.Composefs
		}
		if merged.Flatpaks == nil {
			merged.Flatpaks = parent.Flatpaks
		}
		if merged.DefaultHostname == "" {
			merged.DefaultHostname = parent.DefaultHostname
		}
		if len(merged.Carousel) == 0 {
			merged.Carousel = parent.Carousel
		}
	}
	if len(n.Children) > 0 {
		out := []CatalogLeaf{}
		for _, child := range n.Children {
			out = append(out, child.leaves(&merged)...)
		}
		return out
	}
	imgref := n.imgrefOf(parent)
	if imgref == "" {
		return []CatalogLeaf{}
	}
	needsUser := true
	if merged.NeedsUser != nil {
		needsUser = *merged.NeedsUser
	}
	composefs := false
	if merged.Composefs != nil {
		composefs = *merged.Composefs
	}
	return []CatalogLeaf{{
		Name:              firstNonEmpty(n.Name, imgref),
		Desc:              firstNonEmpty(n.Desc, merged.Desc),
		Imgref:            imgref,
		Filesystem:        firstNonEmpty(n.Filesystem, merged.Filesystem),
		Filesystems:       append([]string{}, merged.Filesystems...),
		NeedsUserCreation: needsUser,
		Bootloader:        firstNonEmpty(n.Bootloader, merged.Bootloader),
		Composefs:         composefs,
		Flatpaks:          n.flatpaksOf(parent),
		DefaultHostname:   firstNonEmpty(n.DefaultHostname, merged.DefaultHostname),
		Carousel:          parseCarousel(merged.Carousel),
	}}
}

func parseCarousel(raw []json.RawMessage) []CarouselSlide {
	out := []CarouselSlide{}
	for _, item := range raw {
		var slide struct {
			Title       string `json:"title"`
			Description string `json:"description"`
			Image       string `json:"image"`
			Resource    string `json:"resource"`
		}
		if err := json.Unmarshal(item, &slide); err != nil {
			continue
		}
		if slide.Image == "" {
			slide.Image = slide.Resource
		}
		if slide.Title == "" && slide.Description == "" && slide.Image == "" {
			continue
		}
		out = append(out, CarouselSlide{Title: slide.Title, Description: slide.Description, Image: slide.Image})
	}
	return out
}

// catalogMain is the `catalog` subcommand: the manifest with inheritance
// resolved, for the QML Source step.
func catalogMain() {
	defaultImage, leaves, err := loadCatalog()
	if err != nil {
		fmt.Fprintf(os.Stderr, "catalog: %v\n", err)
		os.Exit(1)
	}
	result := map[string]any{
		"default_image": defaultImage,
		"leaves":        leaves,
	}
	enc := json.NewEncoder(os.Stdout)
	enc.SetIndent("", "  ")
	if err := enc.Encode(result); err != nil {
		fmt.Fprintf(os.Stderr, "encode output: %v\n", err)
		os.Exit(1)
	}
}
