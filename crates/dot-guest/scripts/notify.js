// Post a notification banner. Input: { title, message }. Output: { ok: true }.
function run(argv) {
  const input = JSON.parse(argv[0]);
  const app = Application.currentApplication();
  app.includeStandardAdditions = true;
  app.displayNotification(input.message, { withTitle: input.title });
  return JSON.stringify({ ok: true });
}
