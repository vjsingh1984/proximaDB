/**
 * ProximaDB Embedded - In-process vector database for Node.js
 *
 * @module proximadb-embedded
 */

const { existsSync, readFileSync } = require('fs');
const { join } = require('path');

const { platform, arch } = process;

let nativeBinding = null;
let localFileExisted = false;
let loadError = null;

function isMusl() {
  // For Node 10
  if (!process.report || typeof process.report.getReport !== 'function') {
    try {
      const lddPath = require('child_process').execSync('which ldd').toString().trim();
      return readFileSync(lddPath, 'utf8').includes('musl');
    } catch (e) {
      return true;
    }
  } else {
    const { glibcVersionRuntime } = process.report.getReport().header;
    return !glibcVersionRuntime;
  }
}

switch (platform) {
  case 'darwin':
    localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.darwin-universal.node'));
    try {
      if (localFileExisted) {
        nativeBinding = require('./proximadb-embedded.darwin-universal.node');
      } else {
        nativeBinding = require('proximadb-embedded-darwin-universal');
      }
      break;
    } catch {}
    switch (arch) {
      case 'x64':
        localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.darwin-x64.node'));
        try {
          if (localFileExisted) {
            nativeBinding = require('./proximadb-embedded.darwin-x64.node');
          } else {
            nativeBinding = require('proximadb-embedded-darwin-x64');
          }
        } catch (e) {
          loadError = e;
        }
        break;
      case 'arm64':
        localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.darwin-arm64.node'));
        try {
          if (localFileExisted) {
            nativeBinding = require('./proximadb-embedded.darwin-arm64.node');
          } else {
            nativeBinding = require('proximadb-embedded-darwin-arm64');
          }
        } catch (e) {
          loadError = e;
        }
        break;
      default:
        throw new Error(`Unsupported architecture on macOS: ${arch}`);
    }
    break;
  case 'linux':
    switch (arch) {
      case 'x64':
        if (isMusl()) {
          localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.linux-x64-musl.node'));
          try {
            if (localFileExisted) {
              nativeBinding = require('./proximadb-embedded.linux-x64-musl.node');
            } else {
              nativeBinding = require('proximadb-embedded-linux-x64-musl');
            }
          } catch (e) {
            loadError = e;
          }
        } else {
          localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.linux-x64-gnu.node'));
          try {
            if (localFileExisted) {
              nativeBinding = require('./proximadb-embedded.linux-x64-gnu.node');
            } else {
              nativeBinding = require('proximadb-embedded-linux-x64-gnu');
            }
          } catch (e) {
            loadError = e;
          }
        }
        break;
      case 'arm64':
        if (isMusl()) {
          localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.linux-arm64-musl.node'));
          try {
            if (localFileExisted) {
              nativeBinding = require('./proximadb-embedded.linux-arm64-musl.node');
            } else {
              nativeBinding = require('proximadb-embedded-linux-arm64-musl');
            }
          } catch (e) {
            loadError = e;
          }
        } else {
          localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.linux-arm64-gnu.node'));
          try {
            if (localFileExisted) {
              nativeBinding = require('./proximadb-embedded.linux-arm64-gnu.node');
            } else {
              nativeBinding = require('proximadb-embedded-linux-arm64-gnu');
            }
          } catch (e) {
            loadError = e;
          }
        }
        break;
      default:
        throw new Error(`Unsupported architecture on Linux: ${arch}`);
    }
    break;
  case 'win32':
    switch (arch) {
      case 'x64':
        localFileExisted = existsSync(join(__dirname, 'proximadb-embedded.win32-x64-msvc.node'));
        try {
          if (localFileExisted) {
            nativeBinding = require('./proximadb-embedded.win32-x64-msvc.node');
          } else {
            nativeBinding = require('proximadb-embedded-win32-x64-msvc');
          }
        } catch (e) {
          loadError = e;
        }
        break;
      default:
        throw new Error(`Unsupported architecture on Windows: ${arch}`);
    }
    break;
  default:
    throw new Error(`Unsupported platform: ${platform}`);
}

if (!nativeBinding) {
  if (loadError) {
    throw loadError;
  }
  throw new Error('Failed to load native binding');
}

const { ProximaDB, version } = nativeBinding;

module.exports = {
  ProximaDB,
  version,
};
