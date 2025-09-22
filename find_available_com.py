#!/usr/bin/env python3
"""
find_available_com.py

Generates random domain labels (4-10 chars), queries the Verisign RDAP endpoint
for .com domains, records available names to available_domains.txt.

Usage:
    python3 find_available_com.py

Configurable via constants below.
"""

import random
import string
import time
import requests
import sys
import logging
from datetime import datetime

# ---------- CONFIG ----------
MIN_LEN = 4
MAX_LEN = 10
SLEEP_SECONDS = 2           # wait time between successful queries (adjust politely)
ERROR_BACKOFF_BASE = 5      # base seconds to wait on error, multiplied exponentially
OUTPUT_FILE = "available_domains.txt"
LOG_FILE = "find_available_com.log"
USE_ONLY_LETTERS = False    # True -> only letters; False -> letters+digits
MAX_ITERATIONS = None       # None => run forever; or set integer for testing
# ----------------------------

# RDAP endpoint for .com (Verisign)
RDAP_TEMPLATE = "https://rdap.verisign.com/com/v1/domain/{domain}"

# Setup logging
logger = logging.getLogger("find_available_com")
logger.setLevel(logging.INFO)
fh = logging.FileHandler(LOG_FILE)
fh.setFormatter(logging.Formatter("%(asctime)s %(levelname)s: %(message)s"))
logger.addHandler(fh)
sh = logging.StreamHandler(sys.stdout)
sh.setFormatter(logging.Formatter("%(asctime)s %(message)s", "%H:%M:%S"))
logger.addHandler(sh)


def random_label(min_len=MIN_LEN, max_len=MAX_LEN, letters_only=USE_ONLY_LETTERS):
    length = random.randint(min_len, max_len)
    return ''.join(random.choices(string.ascii_lowercase, k=length))


def is_domain_available_via_rdap(domain: str, timeout=10):
    """
    Query the Verisign RDAP endpoint for the domain.
    Return True if available (RDAP returns 404), False if registered,
    or raise on other HTTP errors or unexpected responses.
    """
    url = RDAP_TEMPLATE.format(domain=domain)
    headers = {
        "User-Agent": "find-available-com-script/1.0 (+https://example.local/)"
    }
    resp = requests.get(url, headers=headers, timeout=timeout)
    # RDAP: 200 => registered (body with JSON), 404 => not found (available).
    if resp.status_code == 404:
        return True
    if resp.status_code == 200:
        return False
    # For other codes, raise an exception to be handled by caller (rate-limiting, 429, 5xx etc)
    resp.raise_for_status()


def append_available(domain: str):
    entry = f"{domain}  # found {datetime.utcnow().isoformat()}Z\n"
    with open(OUTPUT_FILE, "a", encoding="utf-8") as f:
        f.write(entry)


def main():
    logger.info("Starting domain availability scanner (RDAP).")
    iterations = 0
    backoff_attempts = 0

    try:
        while True:
            if MAX_ITERATIONS is not None and iterations >= MAX_ITERATIONS:
                logger.info("Reached MAX_ITERATIONS, exiting.")
                break
            iterations += 1
            label = random_label()
            domain = f"{label}.com"
            logger.info(f"Checking {domain} ...")

            try:
                available = is_domain_available_via_rdap(domain)
            except requests.HTTPError as e:
                status = getattr(e.response, "status_code", None)
                logger.warning(f"HTTP error for {domain}: {e} (status {status}). Backing off.")
                # Exponential backoff on HTTP errors
                backoff = ERROR_BACKOFF_BASE * (2 ** backoff_attempts)
                backoff_attempts = min(backoff_attempts + 1, 6)  # cap exponent
                time.sleep(backoff)
                continue
            except requests.RequestException as e:
                logger.warning(f"Network/Request error: {e}. Backing off.")
                backoff = ERROR_BACKOFF_BASE * (2 ** backoff_attempts)
                backoff_attempts = min(backoff_attempts + 1, 6)
                time.sleep(backoff)
                continue

            # reset backoff counter on success
            backoff_attempts = 0

            if available:
                logger.info(f"AVAILABLE: {domain}  -> appending to {OUTPUT_FILE}")
                append_available(domain)
            else:
                logger.info(f"TAKEN: {domain}")

            # Polite sleep between requests to avoid triggering rate-limits.
            # For large runs, increase SLEEP_SECONDS or use a paid API.
            time.sleep(SLEEP_SECONDS)

    except KeyboardInterrupt:
        logger.info("Interrupted by user. Exiting.")
    except Exception as e:
        logger.exception(f"Unhandled exception: {e}")
    finally:
        logger.info("Scanner stopped.")


if __name__ == "__main__":
    main()

