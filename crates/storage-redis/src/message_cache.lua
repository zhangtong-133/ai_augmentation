local previous = nil
if redis.call('STRLEN', KEYS[1]) <= 3145728 then
    previous = redis.call('GET', KEYS[1])
end
if previous then
    local ok, decoded = pcall(cjson.decode, previous)
    if ok and type(decoded) == 'table' and type(decoded.revision) == 'number' and decoded.revision >= tonumber(ARGV[2]) then
        return 0
    end
end
redis.call('SET', KEYS[1], ARGV[1], 'EX', 1800)
return 1
