```lua
local M = {}
function M.greet(name)
  if name == nil then return "hi" end
  for i = 1, 10 do print(i) end
  return string.format("Hello %s", name) -- comment
end
return M
```
