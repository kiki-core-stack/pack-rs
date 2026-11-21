-- KEYS[1]: active connection key. ARGV[1]: expected connection identity.
-- Delete only when the stored connection still belongs to the caller.
if redis.call('GET', KEYS[1]) == ARGV[1] then
    redis.call('DEL', KEYS[1])
    return 1
end

return 0
