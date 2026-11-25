#include "update_check.h"

namespace vc {

const char* const kUpdateRepo = "Bio00/Virgil-Virtual-Soundcard";
const char* const kUpdateReleasesApi = "https://api.github.com/repos/Bio00/Virgil-Virtual-Soundcard/releases?per_page=30";
const char* const kUpdateReleasesPage = "https://github.com/Bio00/Virgil-Virtual-Soundcard/releases";

UpdateCheck pick_update(const std::string& current_version, const std::vector<ReleaseInfo>& releases) {
  UpdateCheck r;
  const std::string mine = "v" + current_version;
  const ReleaseInfo* newest = nullptr;
  const ReleaseInfo* current = nullptr;
  for (const auto& rel : releases) {
    if (rel.draft || rel.tag.empty()) continue;
    if (!newest || rel.published_at > newest->published_at) newest = &rel;
    if (rel.tag == mine) current = &rel;
  }
  r.known = current != nullptr;
  if (!newest) {
    r.text = "No published releases were found.";
    return r;
  }
  r.latest = *newest;
  const std::string label = newest->tag.size() > 1 && newest->tag[0] == 'v' ? newest->tag.substr(1) : newest->tag;
  if (newest->tag == mine) {
    r.text = "Virgil " + current_version + " is the latest version.";
  } else if (current && newest->published_at <= current->published_at) {
    r.text = "Virgil " + current_version + " is the latest version.";
  } else {
    r.available = true;
    r.text = current ? "Virgil " + label + " is available (this is " + current_version + ")."
                     : "Virgil " + label + " is the latest published version (this build, " + current_version +
                           ", is not one of the published releases).";
  }
  return r;
}

std::string parse_sha256_digest(const std::string& digest) {
  const std::string prefix = "sha256:";
  if (digest.size() != prefix.size() + 64 || digest.compare(0, prefix.size(), prefix) != 0) return "";
  std::string hex = digest.substr(prefix.size());
  for (char& c : hex) {
    if (c >= 'A' && c <= 'F') c = char(c - 'A' + 'a');
    if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return "";
  }
  return hex;
}

bool mac_installer(const ReleaseInfo& release, ReleaseAsset* out) {
  const std::string from = std::string("https://github.com/") + kUpdateRepo + "/releases/download/";
  const std::string suffix = "-macos.pkg";
  for (const auto& a : release.assets) {
    if (a.name.size() <= suffix.size() || a.name.compare(a.name.size() - suffix.size(), suffix.size(), suffix) != 0)
      continue;
    if (a.url.compare(0, from.size(), from) != 0 || a.sha256.size() != 64) continue;
    if (a.size <= 0 || a.size > 200 * 1024 * 1024) continue;
    *out = a;
    return true;
  }
  return false;
}

}  // namespace vc
