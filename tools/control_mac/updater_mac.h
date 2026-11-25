// Downloads a release's installer package, checks it and installs it (macOS).
#pragma once

#include <string>

#include "update_check.h"

namespace vcmac {

// Shows a progress window, downloads `asset`, checks its size and SHA-256,
// asks for an administrator's password, installs it and reopens Virgil
// Control. `page` is the release page offered when something fails. If an
// update is already under way its window comes forward. Main thread only.
void install_update(const vc::ReleaseAsset& asset, const std::string& version, const std::string& page);

// An update is being downloaded or installed.
bool update_in_progress();

}  // namespace vcmac
