'use strict';

const fs = require('fs');
const https = require('https');
const { Transform } = require('stream');
const { pipeline } = require('stream/promises');

const DOWNLOAD_TIMEOUT_MS = 30000;
const MAX_DOWNLOAD_BYTES = 256 * 1024 * 1024;
const MAX_REDIRECTS = 5;

/** Download HTTPS only, with a per-request deadline, redirect and byte limits. */
function download(url, destination, options = {}, redirects = 0) {
  return new Promise((resolve, reject) => {
    let responseBody;
    const parsed = new URL(url);
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password) {
      reject(new Error('release download requires HTTPS without embedded credentials'));
      return;
    }
    const request = https.get(parsed, (response) => {
      if ([301, 302, 303, 307, 308].includes(response.statusCode)) {
        response.destroy();
        clearTimeout(timer);
        if (redirects >= MAX_REDIRECTS || !response.headers.location) {
          reject(new Error('release download redirect limit or missing location'));
          return;
        }
        // [Review Fix #NpmRedirect] Reject malformed Location values through the promise.
        redirectUrl(response.headers.location, parsed)
          .then((nextUrl) => download(nextUrl, destination, options, redirects + 1))
          .then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.destroy();
        clearTimeout(timer);
        reject(new Error(`release download failed (${response.statusCode})`));
        return;
      }
      responseBody = response;
      saveResponse(response, destination, options.maxBytes || MAX_DOWNLOAD_BYTES)
        .then(resolve, reject).finally(() => clearTimeout(timer));
    });
    const timer = setTimeout(() => {
      request.destroy(new Error('release download timed out'));
    }, options.timeoutMs || DOWNLOAD_TIMEOUT_MS);
    request.on('error', (error) => {
      clearTimeout(timer);
      // [Review Fix #NpmDownloadClose] Let pipeline close the file before cleanup.
      if (responseBody) responseBody.destroy(error);
      else reject(error);
    });
  });
}

/** Convert parser errors into a handled rejection without logging server URLs. */
async function redirectUrl(location, base) {
  try { return new URL(location, base); } catch (error) {
    throw new Error('invalid release redirect', { cause: error });
  }
}

/** Stream a response without buffering an unbounded archive in memory. */
async function saveResponse(response, destination, maxBytes) {
  let receivedBytes = 0;
  const limit = new Transform({
    transform(chunk, encoding, callback) {
      receivedBytes += chunk.length;
      callback(receivedBytes > maxBytes ? new Error('release download exceeds size limit') : null,
        chunk);
    },
  });
  await pipeline(response, limit, fs.createWriteStream(destination, { flags: 'wx' }));
}

module.exports = { download };
