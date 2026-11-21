-- KEYS[1]: active connection key.
-- ARGV[1]: expected connection identity. ARGV[2]: TTL in seconds.
local current = redis.call('GET', KEYS[1])
if current == ARGV[1] then
    return redis.call('EXPIRE', KEYS[1], ARGV[2])
end

if current == false then
    redis.call('SET', KEYS[1], ARGV[1], 'EX', ARGV[2])
    return 1
end

return 0
