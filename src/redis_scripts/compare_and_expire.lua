-- KEYS[1]: active connection key.
-- ARGV[1]: expected connection identity. ARGV[2]: TTL in seconds.
if redis.call('GET', KEYS[1]) == ARGV[1] then
    return redis.call('EXPIRE', KEYS[1], ARGV[2])
end

return 0
