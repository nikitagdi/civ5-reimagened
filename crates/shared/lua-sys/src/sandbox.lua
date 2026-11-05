-- Run once by Lua::sandbox with (rawload, getfenv, setfenv, type, error, concat).
-- Installs the environment-aware loadstring and load that replace the standard ones.
local rawload, getfenv, setfenv, type, error, concat = ...

function loadstring(src, name)
	if type(src) ~= "string" then
		error("bad argument #1 to 'loadstring' (string expected, got " .. type(src) .. ")", 2)
	end
	local f, err = rawload(src, name)
	if f then
		setfenv(f, getfenv(2))
	end
	return f, err
end

function load(reader, name)
	if type(reader) ~= "function" then
		error("bad argument #1 to 'load' (function expected, got " .. type(reader) .. ")", 2)
	end
	local parts = {}
	while true do
		local piece = reader()
		if piece == nil or piece == "" then
			break
		end
		parts[#parts + 1] = piece
	end
	local f, err = rawload(concat(parts), name or "=(load)")
	if f then
		setfenv(f, getfenv(2))
	end
	return f, err
end
