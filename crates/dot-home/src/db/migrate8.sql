-- MCP servers that apps on the Mac provide (docs/home/TOOLS.md "App servers"): the last
-- servers the Mac reported, with the tools of the ones the owner turned on.
ALTER TABLE instances ADD COLUMN mcp_cache TEXT;
-- For app servers: what the server was when the owner turned it on. A server whose
-- command or environment changed since counts as off until it's turned on again.
ALTER TABLE permissions ADD COLUMN fingerprint TEXT;
