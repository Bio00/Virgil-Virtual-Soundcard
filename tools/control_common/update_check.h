// Update check against the GitHub releases of the repository the builds are
// published from (as StageRack and similar apps do). The apps fetch the
// release list (kUpdateReleasesApi) and ask pick_update what to tell the user.
#pragma once

#include <string>
#include <vector>

namespace vc {

// Where this build's releases are published.
extern const char* const kUpdateRepo;          // "owner/name"
extern const char* const kUpdateReleasesApi;   // GitHub API: the release list (JSON)
extern const char* const kUpdateReleasesPage;  // the releases page, for the browser

struct ReleaseInfo {
  std::string tag;           // "v0.8.3-lowlat"
  std::string name;          // release title
  std::string url;           // its page (html_url)
  std::string published_at;  // ISO 8601, UTC ("2026-11-11T03:23:23Z"): sorts as text
  bool draft = false;
  bool prerelease = false;
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

}  // namespace vc
