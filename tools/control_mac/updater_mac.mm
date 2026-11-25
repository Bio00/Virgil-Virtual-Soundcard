#import "updater_mac.h"

#import <AppKit/AppKit.h>
#import <CommonCrypto/CommonDigest.h>
#include <unistd.h>

namespace {

// Run as root with the package and its SHA-256. The download is in a folder
// the user's other programs can write to, so it is copied where only root can
// and checked there: what is installed is exactly what was checked.
NSString* const kInstallScript =
    @"set -e\n"
    @"umask 077\n"
    @"d=$(/usr/bin/mktemp -d /private/var/root/virgil-update.XXXXXX)\n"
    @"trap '/bin/rm -rf \"$d\"' EXIT\n"
    @"/bin/cp \"$1\" \"$d/Virgil.pkg\"\n"
    @"got=$(/usr/bin/shasum -a 256 \"$d/Virgil.pkg\" | /usr/bin/cut -c1-64)\n"
    @"[ \"$got\" = \"$2\" ] || { echo 'The downloaded file changed before it was installed.' >&2; exit 65; }\n"
    @"/usr/sbin/installer -pkg \"$d/Virgil.pkg\" -target / 2>&1\n";

NSString* const kPrivileged =
    @"on run argv\n"
    @"  do shell script \"/bin/sh -c \" & quoted form of (item 1 of argv) & \" sh \" & quoted form of (item 2 of "
    @"argv) & \" \" & quoted form of (item 3 of argv) with prompt \"Virgil Control wants to install an update.\" "
    @"with administrator privileges\n"
    @"end run\n";

std::string sha256(NSData* data) {
  unsigned char md[CC_SHA256_DIGEST_LENGTH];
  CC_SHA256(data.bytes, CC_LONG(data.length), md);
  static const char* hex = "0123456789abcdef";
  std::string s;
  for (unsigned char c : md) {
    s += hex[c >> 4];
    s += hex[c & 15];
  }
  return s;
}

}  // namespace

@interface VCUpdater : NSObject <NSURLSessionDownloadDelegate>
@end

static VCUpdater* g_updater = nil;

@implementation VCUpdater {
  vc::ReleaseAsset _asset;
  std::string _page;
  NSString* _version;
  NSString* _folder;  // private, holds the downloaded package
  NSPanel* _panel;
  NSTextField* _label;
  NSProgressIndicator* _bar;
  NSButton* _cancel;
  NSURLSession* _session;
  NSString* _problem;
}

- (instancetype)initWithAsset:(const vc::ReleaseAsset&)asset version:(NSString*)version page:(const std::string&)page {
  if ((self = [super init])) {
    _asset = asset;
    _version = version;
    _page = page;
  }
  return self;
}

- (void)start {
  _panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 400, 104)
                                      styleMask:NSWindowStyleMaskTitled
                                        backing:NSBackingStoreBuffered
                                          defer:NO];
  _panel.title = @"Virgil update";
  _panel.releasedWhenClosed = NO;
  _panel.hidesOnDeactivate = NO;
  _label = [NSTextField labelWithString:[NSString stringWithFormat:@"Downloading Virgil %@…", _version]];
  _label.frame = NSMakeRect(20, 70, 360, 18);
  _bar = [[NSProgressIndicator alloc] initWithFrame:NSMakeRect(20, 46, 360, 18)];
  _bar.indeterminate = NO;
  _bar.minValue = 0;
  _bar.maxValue = double(_asset.size);
  _cancel = [NSButton buttonWithTitle:@"Cancel" target:self action:@selector(cancel:)];
  _cancel.frame = NSMakeRect(290, 8, 96, 32);
  for (NSView* v in @[ _label, _bar, _cancel ]) [_panel.contentView addSubview:v];
  [_panel center];
  [self bringForward];

  _session = [NSURLSession sessionWithConfiguration:[NSURLSessionConfiguration ephemeralSessionConfiguration]
                                           delegate:self
                                      delegateQueue:[NSOperationQueue mainQueue]];
  NSMutableURLRequest* req = [NSMutableURLRequest requestWithURL:[NSURL URLWithString:@(_asset.url.c_str())]];
  req.timeoutInterval = 30;
  [req setValue:@"Virgil-Control" forHTTPHeaderField:@"User-Agent"];
  [[_session downloadTaskWithRequest:req] resume];
}

- (void)bringForward {
  [NSApp activateIgnoringOtherApps:YES];
  [_panel makeKeyAndOrderFront:nil];
}

- (void)cancel:(id)sender {
  _cancel.enabled = NO;
  [_session invalidateAndCancel];
}

- (void)URLSession:(NSURLSession*)s
                 downloadTask:(NSURLSessionDownloadTask*)t
                 didWriteData:(int64_t)n
            totalBytesWritten:(int64_t)total
    totalBytesExpectedToWrite:(int64_t)expected {
  _bar.doubleValue = double(total);
  if (total > _asset.size) {
    _problem = @"The download is larger than the published file.";
    [t cancel];
  }
}

- (void)URLSession:(NSURLSession*)s
                 downloadTask:(NSURLSessionDownloadTask*)t
    didFinishDownloadingToURL:(NSURL*)file {
  // `file` is gone when this returns: check and keep it now.
  const NSInteger code =
      [t.response isKindOfClass:[NSHTTPURLResponse class]] ? ((NSHTTPURLResponse*)t.response).statusCode : 0;
  if (code != 200) {
    _problem = [NSString stringWithFormat:@"GitHub answered %ld.", long(code)];
    return;
  }
  NSData* data = [NSData dataWithContentsOfURL:file];
  if (!data || int64_t(data.length) != _asset.size || sha256(data) != _asset.sha256) {
    _problem = @"The downloaded file is not the published one (its size or checksum differs). Nothing was installed.";
    return;
  }
  char tmpl[1024];
  snprintf(tmpl, sizeof tmpl, "%s/virgil-update.XXXXXX", NSTemporaryDirectory().fileSystemRepresentation);
  if (!mkdtemp(tmpl)) {
    _problem = @"The update could not be saved.";
    return;
  }
  _folder = [[NSFileManager defaultManager] stringWithFileSystemRepresentation:tmpl length:strlen(tmpl)];
  if (![data writeToFile:[self package] atomically:YES]) _problem = @"The update could not be saved.";
}

- (NSString*)package {
  return [_folder stringByAppendingPathComponent:@"Virgil.pkg"];
}

- (void)URLSession:(NSURLSession*)s task:(NSURLSessionTask*)t didCompleteWithError:(NSError*)error {
  [_session finishTasksAndInvalidate];
  if (!_problem && error) {
    if (error.code == NSURLErrorCancelled) {
      [self finish:nil];
      return;
    }
    _problem = error.localizedDescription;
  }
  if (_problem) {
    [self finish:_problem];
    return;
  }
  [self install];
}

- (void)install {
  _label.stringValue = [NSString stringWithFormat:@"Installing Virgil %@…", _version];
  _cancel.hidden = YES;
  _bar.indeterminate = YES;
  [_bar startAnimation:nil];
  NSTask* task = [[NSTask alloc] init];
  task.executableURL = [NSURL fileURLWithPath:@"/usr/bin/osascript"];
  task.arguments = @[ @"-e", kPrivileged, kInstallScript, [self package], @(_asset.sha256.c_str()) ];
  task.standardInput = [NSFileHandle fileHandleWithNullDevice];
  NSPipe* pipe = [NSPipe pipe];
  task.standardOutput = pipe;
  task.standardError = pipe;
  dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0), ^{
    NSError* err = nil;
    NSString* out = @"";
    int status = -1;
    if ([task launchAndReturnError:&err]) {
      NSData* d = [pipe.fileHandleForReading readDataToEndOfFile];
      [task waitUntilExit];
      status = task.terminationStatus;
      out = [[NSString alloc] initWithData:d encoding:NSUTF8StringEncoding] ?: @"";
    } else {
      out = err.localizedDescription ?: @"";
    }
    dispatch_async(dispatch_get_main_queue(), ^{
      if (status == 0) [self relaunch];
      else if ([out containsString:@"(-128)"]) [self finish:nil];  // password dialog cancelled
      else [self finish:[@"The installer reported a problem:\n\n" stringByAppendingString:out]];
    });
  });
}

// The app on disk is now the new one: open it once this one has gone.
- (void)relaunch {
  [self cleanUp];
  NSTask* t = [[NSTask alloc] init];
  t.executableURL = [NSURL fileURLWithPath:@"/bin/sh"];
  t.arguments = @[
    @"-c", @"while /bin/kill -0 \"$1\" 2>/dev/null; do /bin/sleep 0.2; done; exec /usr/bin/open -a \"$2\"", @"sh",
    [NSString stringWithFormat:@"%d", getpid()], @"/Applications/Virgil Control.app"
  ];
  [t launchAndReturnError:nil];
  [NSApp terminate:nil];
}

- (void)cleanUp {
  [_panel orderOut:nil];
  if (_folder) [[NSFileManager defaultManager] removeItemAtPath:_folder error:nil];
  _folder = nil;
}

- (void)finish:(NSString*)problem {
  [self cleanUp];
  const std::string page = _page;
  g_updater = nil;
  if (!problem) return;
  // Not in this main-queue block: a modal run here would hold back the queue.
  CFRunLoopPerformBlock(CFRunLoopGetMain(), kCFRunLoopCommonModes, ^{
    NSAlert* a = [[NSAlert alloc] init];
    a.messageText = @"The update was not installed";
    a.informativeText = problem;
    [a addButtonWithTitle:@"OK"];
    [a addButtonWithTitle:@"Open release page"];
    [NSApp activateIgnoringOtherApps:YES];
    if ([a runModal] == NSAlertSecondButtonReturn)
      [[NSWorkspace sharedWorkspace] openURL:[NSURL URLWithString:@(page.c_str())]];
  });
  CFRunLoopWakeUp(CFRunLoopGetMain());
}
@end

namespace vcmac {

void install_update(const vc::ReleaseAsset& asset, const std::string& version, const std::string& page) {
  if (g_updater) {
    [g_updater bringForward];
    return;
  }
  g_updater = [[VCUpdater alloc] initWithAsset:asset version:@(version.c_str()) page:page];
  [g_updater start];
}

bool update_in_progress() { return g_updater != nil; }

}  // namespace vcmac
