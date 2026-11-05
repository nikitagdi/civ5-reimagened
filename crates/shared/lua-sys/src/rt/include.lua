-- Run once per state by the runtime with (resolve, getfenv, type, error).
-- `resolve(env, name, wildcard)` returns the list of paths and the list of loaded chunks
-- (already set to run in `env`); this function runs them in the caller's context.
local resolve, getfenv, type, error = ...

function include(name, wildcard)
	if type(name) ~= "string" then
		error("bad argument #1 to 'include' (string expected, got " .. type(name) .. ")", 2)
	end
	local env = getfenv(2)
	local paths, chunks = resolve(env, name, wildcard and true or false)
	if not paths then
		error(chunks, 2) -- chunks holds the message; blame the caller of include
	end
	for i = 1, #chunks do
		chunks[i]()
	end
	if wildcard then
		return paths
	end
end
