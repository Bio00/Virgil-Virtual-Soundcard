#!/bin/sh
# Installs the Virgil Core Audio device from this folder.
cd "$(dirname "$0")" || exit 1
echo "Installing VirgilAudio.driver into /Library/Audio/Plug-Ins/HAL (needs your password)..."
if ! sudo rm -rf /Library/Audio/Plug-Ins/HAL/VirgilAudio.driver ||
	! sudo cp -R VirgilAudio.driver /Library/Audio/Plug-Ins/HAL/; then
	echo "Installing the driver failed (see above). Nothing was changed in Core Audio."
	exit 1
fi
sudo xattr -dr com.apple.quarantine /Library/Audio/Plug-Ins/HAL/VirgilAudio.driver 2>/dev/null
sudo killall coreaudiod 2>/dev/null
# macOS can leave AirPlayXPCHelper re-registering its driver with the new
# coreaudiod in a loop (thousands of times a minute), which keeps coreaudiod
# and every audio client busy and makes apps hang when they open audio.
# Restarting it (launchd relaunches it) breaks the loop.
sleep 2
sudo killall AirPlayXPCHelper 2>/dev/null
echo "Done. \"Virgil Virtual Soundcard\" is now available. You can close this window."
