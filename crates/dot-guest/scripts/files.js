// Move a file or folder to the Trash (recoverable), through NSFileManager so no Finder
// Automation permission is needed. Input: { path }. Output: { trashed: path }.
ObjC.import('Foundation');

function fail(code, message) {
  throw new Error(code + ': ' + message);
}

function run(argv) {
  const input = JSON.parse(argv[0]);
  const fm = $.NSFileManager.defaultManager;
  const url = $.NSURL.fileURLWithPath(input.path);
  // `$()` is a nil box the bridge can write the NSError into; a bare Ref() crashes here.
  const error = $();
  const ok = fm.trashItemAtURLResultingItemURLError(url, null, error);
  if (!ok) {
    const message = error.isNil() ? 'unknown error' : error.localizedDescription.js;
    const code = error.isNil() ? 0 : Number(error.code);
    // NSFileNoSuchFileError 4, NSFileReadNoSuchFileError 260
    if (code === 4 || code === 260) fail('NOT_FOUND', message);
    // NSFileWriteNoPermissionError 513, NSFileReadNoPermissionError 257
    if (code === 513 || code === 257) fail('PERMISSION_DENIED', message);
    fail('FAILED', message);
  }
  return JSON.stringify({ trashed: input.path });
}
