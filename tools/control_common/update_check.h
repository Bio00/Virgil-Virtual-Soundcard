// Update check against the GitHub releases of the repository the builds are
// published from (as StageRack and similar apps do). The apps fetch the
// release list (kUpdateReleasesApi) and ask pick_update what to tell the user.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace vc {

// Where this build's releases are published.
extern const char* const kUpdateRepo;          // "owner/name"
extern const char* const kUpdateReleasesApi;   // GitHub API: the release list (JSON)
extern const char* const kUpdateReleasesPage;  // the releases page, for the browser

// A file of a release.
struct ReleaseAsset {
  std::string name;    // "Virgil-0.8.4-macos.pkg"
  std::string url;     // browser_download_url
  std::string sha256;  // lower-case hex (GitHub's "digest"), empty if unknown
  int64_t size = 0;
};

struct ReleaseInfo {
  std::string tag;           // "v0.8.3-lowlat"
  std::string name;          // release title
  std::string url;           // its page (html_url)
  std::string published_at;  // ISO 8601, UTC ("2026-11-11T03:23:23Z"): sorts as text
  bool draft = false;
  bool prerelease = false;
  std::vector<ReleaseAsset> assets;
};

struct UpdateCheck {
  bool available = false;  // a release newer than this build
  bool known = false;      // this build is one of the published releases
  ReleaseInfo latest;      // the newest published release (empty tag: none)
  std::string text;        // what to tell the user
};

// current_version: VIRGIL_VERSION ("0.8.3-lowlat"); its release is tagged
// "v" + it. The newest release (by publication time, drafts left out) is an
// update when it is not this build and was published after it (or when this
// build is not a published release at all, e.g. built from source).
UpdateCheck pick_update(const std::string& current_version, const std::vector<ReleaseInfo>& releases);

// GitHub's asset digest ("sha256:<64 hex digits>") as lower-case hex; empty
// when it is anything else.
std::string parse_sha256_digest(const std::string& digest);

// The macOS installer of a release that Virgil Control can install by itself:
// published in kUpdateRepo, with a checksum and a sensible size. False: none.
bool mac_installer(const ReleaseInfo& release, ReleaseAsset* out);

}  // namespace vc
