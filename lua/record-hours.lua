local M = {}

local next_emit = 0
local interval = 2

local function on_key()
    local now = os.time()
    if now > next_emit then
        next_emit = now + interval
        local file = os.getenv("HOME") .. "/.local/state/hours.log.json"
        vim.system({ "record-hours", "--file", file, "record" })
    end
end

function M.register()
    local namespace = vim.on_key(on_key, 0)
end

return M
