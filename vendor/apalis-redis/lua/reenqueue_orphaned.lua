-- KEYS[1]: the worker set
-- KEYS[2]: the active job list
-- KEYS[3]: the signal list
-- KEYS[4]: The workers key prefix
-- ARGV[1]: the timestamp before which a worker is considered expired
-- ARGV[2]: restrict to this specific worker; empty string means all expired workers
-- ARGV[3]: maximum recovered jobs and candidate workers per call
-- Returns: number of jobs processed
local worker_key_prefix = KEYS[4]
local budget = tonumber(ARGV[3])

local workers
if ARGV[2] and ARGV[2] ~= '' then
    -- Only consider this worker if it is in fact expired
    local score = redis.call("zscore", KEYS[1], ARGV[2])
    if score and tonumber(score) <= tonumber(ARGV[1]) then
        workers = {ARGV[2]}
    else
        workers = {}
    end
else
    workers = redis.call("zrangebyscore", KEYS[1], 0, ARGV[1], 'LIMIT', 0, budget)
end

redis.replicate_commands()

local processed = 0

for _, worker in ipairs(workers) do
    local inflight_set_key = worker_key_prefix .. ":" .. worker
    local jobs = redis.call("srandmember", inflight_set_key, budget - processed)
    local count = table.getn(jobs)

    if count > 0 then
        redis.call("rpush", KEYS[2], unpack(jobs))
        redis.call("srem", inflight_set_key, unpack(jobs))
        processed = processed + count
    end

    -- Keep a partially recovered worker discoverable by the next bounded pass.
    if redis.call("scard", inflight_set_key) == 0 then
        redis.call("zrem", KEYS[1], worker)
    end
    if processed >= budget then
        break
    end
end

if processed > 0 then
    redis.call("del", KEYS[3])
    redis.call("lpush", KEYS[3], 1)
end

return processed
