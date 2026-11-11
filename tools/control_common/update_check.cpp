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

}  // namespace vc
