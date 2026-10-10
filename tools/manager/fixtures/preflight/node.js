#!/usr/bin/env node
'use strict';

// Production --redstone-node-path seam: ignore the bundle argument and speak
// the bridge protocol directly, using only Node's built-in modules.
// old.hex/new.hex are copied verbatim from contract/redstone-adapter/tests/test.rs
// case::stellar (1770985144000 ms) / case::js_sdk (1771336150000 ms).
// These are signed ETH/BTC packages, not newly signed or mocked verifier output.
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');
const directory = __dirname;
const socketIndex = process.argv.indexOf('--socket');
if (socketIndex < 0 || !process.argv[socketIndex + 1]) {
    throw new Error('missing --socket');
}
const socketPath = process.argv[socketIndex + 1];
const stat = fs.readFileSync('/proc/self/stat', 'utf8');
const startTime = stat.slice(stat.lastIndexOf(')') + 2).trim().split(/\s+/)[19];
fs.appendFileSync(path.join(directory, 'pids.jsonl'), JSON.stringify({
    pid: process.pid,
    start_time: startTime,
}) + '\n');

// All waits are bounded even if the production bridge or test goes away.
const deadline = Date.now() + 10000;
const lifetime = setTimeout(() => process.exit(1), 600000);
let connection;
let retry;
let connected = false;
const startup = setTimeout(() => stop(1), 10000);
let requestDeadline;
function stop(code) {
    clearTimeout(lifetime);
    clearTimeout(retry);
    clearTimeout(startup);
    clearTimeout(requestDeadline);
    if (connection) connection.destroy();
    process.exit(code);
}
process.on('SIGTERM', () => stop(0));
process.on('SIGINT', () => stop(0));
function connect() {
    connection = net.createConnection(socketPath);
    const attempt = connection;
    connection.setTimeout(60000, () => stop(1));
    let input = '';
    connection.on('connect', () => {
        connected = true;
        clearTimeout(startup);
    });
    connection.on('error', (error) => {
        if (!connected && Date.now() < deadline &&
            (error.code === 'ENOENT' || error.code === 'ECONNREFUSED')) {
            retry = setTimeout(connect, 25);
        } else {
            stop(1);
        }
    });
    connection.on('end', () => stop(0));
    connection.on('close', () => {
        if (connection === attempt && connected) stop(0);
    });
    connection.on('data', (chunk) => {
        input += chunk.toString('utf8');
        if (!requestDeadline) requestDeadline = setTimeout(() => stop(1), 10000);
        if (input.length > 1048576) stop(1);
        let newline;
        while ((newline = input.indexOf('\n')) !== -1) {
            const line = input.slice(0, newline);
            input = input.slice(newline + 1);
            try {
                const request = JSON.parse(line);
                if (!Number.isInteger(request.id) || request.id < 0 ||
                    request.id > 0xffffffff) throw new Error('invalid request id');
                fs.appendFileSync(path.join(directory, 'requests.jsonl'), line + '\n');
                const config = JSON.parse(fs.readFileSync(path.join(directory, 'config.json'), 'utf8'));
                const response = config.failure
                    ? { id: request.id, status: 'failure', message: 'controlled preflight fixture failure' }
                    : { id: request.id, status: 'success', data: fs.readFileSync(path.join(directory, 'new.hex'), 'utf8').trim() };
                const writeDeadline = setTimeout(() => stop(1), 10000);
                connection.write(JSON.stringify(response) + '\n', (error) => {
                    clearTimeout(writeDeadline);
                    if (error) stop(1);
                });
            } catch (error) {
                console.error(error);
                stop(1);
            }
        }
        if (!input.length) {
            clearTimeout(requestDeadline);
            requestDeadline = undefined;
        }
    });
}
connect();
