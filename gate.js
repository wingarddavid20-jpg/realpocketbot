#!/usr/bin/env node
// ============================================================
//  call-gate  v1.0.0
//  Memecoin pre-trade filtration gate
//  Usage:  node gate.js <token_address_or_messy_text>
//          node gate.js MOCK_PASS
//          node gate.js MOCK_FAIL
// ============================================================

'use strict';

require('dotenv').config();
const axios  = require('axios');
const crypto = require('crypto');

// EIP-55 checksum address — Blockscout requires this format
// We use the ethers-compatible keccak256 via Buffer XOR trick
function toChecksumAddress(addr) {
  const address = addr.toLowerCase().replace('0x', '');
  // Build a simple keccak256 via the 'keccak' npm or fall back to uppercase if unavailable
  try {
    const { keccak256 } = require('js-sha3');
    const hash = keccak256(address);
    return '0x' + address.split('').map((c, i) =>
      parseInt(hash[i], 16) >= 8 ? c.toUpperCase() : c
    ).join('');
  } catch (_) {
    // If js-sha3 not installed, return mixed-case — Blockscout usually accepts it
    return '0x' + address;
  }
}

// GMGN-supported chain slugs (hype/hyperliquid not yet supported by gmgn-cli)
const GMGN_SUPPORTED_CHAINS = new Set(['sol', 'bsc', 'base', 'eth', 'arc', 'stable', 'solana']);

// ─────────────────────────────────────────────────────────────
//  CONSTANTS & CONFIG
// ─────────────────────────────────────────────────────────────
const GMGN_API_KEY        = process.env.GMGN_API_KEY        || '';
const SOLANA_RPC_URL      = process.env.SOLANA_RPC_URL      || 'https://api.mainnet-beta.solana.com';
const EVM_RPC_URL         = process.env.EVM_RPC_URL         || '';
const ROBINHOOD_CHAIN_RPC = process.env.ROBINHOOD_CHAIN_RPC_URL || '';
const LOXLEY_DIR          = process.env.LOXLEY_DIR          || '';

const RULE = {
  BUNDLER_MAX_PCT:       20,   // Rule 1  – bundler wallets hold > 20 %
  TOP10_MAX_PCT:         40,   // Rule 2  – top-10 wallets hold > 40 %
  DEV_HOLD_MAX_PCT:       5,   // Rule 3  – dev wallet holds > 5 %
  LIQ_MIN_USD:        10_000,  // Rule 4  – liquidity < $10 k
  LIQ_MIN_MC_PCT:        10,   // Rule 5  – liquidity < 10 % of MC
  NEW_TOKEN_MAX_AGE_S:  120,   // Rule 11 – token < 2 min old with excessive MC
  NEW_TOKEN_MC_LIMIT: 5_000_000, // excessive MC threshold for brand-new token (Rule 11)
};

// GAMBLE thresholds — failing these means SKIP. Passing these but failing STRICT means GAMBLE.
const LOOSE_RULE = {
  BUNDLER_MAX_PCT:       35,   // Loose Rule 1
  TOP10_MAX_PCT:         60,   // Loose Rule 2
  DEV_HOLD_MAX_PCT:      10,   // Loose Rule 3
  LIQ_MIN_USD:         3_000,  // Loose Rule 4
  LIQ_MIN_MC_PCT:         5,   // Loose Rule 5
};

// ─────────────────────────────────────────────────────────────
//  ADDRESS EXTRACTOR  (CLI input / messy Telegram dump)
// ─────────────────────────────────────────────────────────────

/**
 * Parse the first recognisable token address from arbitrary text.
 * Supports:
 *   • Solana  – Base58, 32-44 chars, avoids [0OIl]
 *   • EVM     – 0x followed by 40 hex chars
 *
 * @param {string} text
 * @returns {{ address: string, chain: 'solana'|'evm'|null }}
 */
function extractAddress(text) {
  if (!text) return { address: null, chain: null };

  let manualChain = null;
  const tLow = text.toLowerCase();
  if (tLow.includes('robinhood')) manualChain = 'robinhood';
  else if (tLow.includes('hype')) manualChain = 'hype';

  // EVM first (unambiguous prefix)
  const evmMatch = text.match(/\b(0x[0-9a-fA-F]{40})\b/);
  if (evmMatch) return { address: evmMatch[1], chain: manualChain || 'evm' };

  // Solana Base58 — 32-44 chars, alphabet excludes 0, O, I, l
  const solMatch = text.match(/\b([1-9A-HJ-NP-Za-km-z]{32,44})\b/);
  if (solMatch) return { address: solMatch[1], chain: manualChain || 'solana' };

  return { address: null, chain: null };
}

// ─────────────────────────────────────────────────────────────
//  EVM CHAIN AUTO-DETECTOR
//  1. GMGN race (for supported chains with liquidity)
//  2. Direct RPC eth_getCode probe (for new chains like Robinhood/Hype)
//  Falls back to eth only if everything fails.
// ─────────────────────────────────────────────────────────────
const EVM_CHAINS = ['eth', 'bsc', 'base', 'robinhood', 'hype', 'hyperliquid'];

// Public RPCs for chains GMGN doesn't support — used to check if a contract exists
const RPC_PROBE_CHAINS = {
  'robinhood': 'https://rpc.mainnet.chain.robinhood.com',
  'hype':      'https://rpc.hyperliquid.xyz/evm',
};

async function probeRpcForContract(address, chain, rpcUrl) {
  const res = await axios.post(rpcUrl, {
    jsonrpc: '2.0', id: 1, method: 'eth_getCode', params: [address, 'latest']
  }, { timeout: 4000 });
  const code = res.data?.result;
  if (code && code !== '0x' && code.length > 4) return chain;
  throw new Error(`No contract on ${chain}`);
}

async function detectEvmChain(address) {
  try {
    return await Promise.any([
      // GMGN race — works for ETH, BSC, Base
      ...EVM_CHAINS.map(c =>
        gmgnCli('info', c, address).then(data => {
          if (Number(data?.liquidity || 0) > 0) return c;
          throw new Error('No liquidity');
        })
      ),
      // Direct RPC probe — works for Robinhood, Hype (even brand new tokens)
      ...Object.entries(RPC_PROBE_CHAINS).map(([chain, rpcUrl]) =>
        probeRpcForContract(address, chain, rpcUrl)
      ),
    ]);
  } catch {
    return 'eth'; // fallback
  }
}

// ─────────────────────────────────────────────────────────────
//  HELPERS
// ─────────────────────────────────────────────────────────────

/**
 * Convert a duration in seconds to a human-readable string.
 * e.g. 90061 → "1d 1h 1m 1s"
 */
function formatAge(seconds) {
  const s = Math.floor(Math.max(0, seconds));
  if (s === 0) return 'unknown';
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const parts = [];
  if (d) parts.push(`${d}d`);
  if (h) parts.push(`${h}h`);
  if (m) parts.push(`${m}m`);
  if (sec || parts.length === 0) parts.push(`${sec}s`);
  return parts.join(' ');
}

// ─────────────────────────────────────────────────────────────
//  OUTPUT FORMATTING
// ─────────────────────────────────────────────────────────────
function printResult({ verdict, address, chain, reasons, metrics }) {
  const border = '═'.repeat(58);
  const thin   = '─'.repeat(58);

  console.log(`\n╔${border}╗`);
  console.log(`║  CALL-GATE  ·  Pre-Trade Filter                          ║`);
  console.log(`╠${border}╣`);
  console.log(`║  ADDRESS : ${(address || 'N/A').padEnd(46)} ║`);
  console.log(`║  CHAIN   : ${(chain   || 'N/A').padEnd(46)} ║`);
  console.log(`╠${border}╣`);

  if (metrics && Object.keys(metrics).length) {
    console.log(`║  METRICS                                                  ║`);
    for (const [k, v] of Object.entries(metrics)) {
      const line = `    ${k}: ${v}`.padEnd(57);
      console.log(`║${line}║`);
    }
    console.log(`╠${border}╣`);
  }

  if (verdict === 'BUY') {
    console.log(`║  VERDICT : 🟢  BUY — safe to proceed                     ║`);
  } else if (verdict === 'GAMBLE') {
    console.log(`║  VERDICT : 🟡  GAMBLE — small size only                  ║`);
  } else {
    console.log(`║  VERDICT : 🚫  SKIP — do not trade                       ║`);
  }
  
  if (reasons && reasons.length) {
      console.log(`╠${border}╣`);
      console.log(`║  WHY:                                                     ║`);
      reasons.forEach(r => {
        // Word-wrap at 54 chars
        const words = r.split(' ');
        let line = '    ';
        for (const word of words) {
          if ((line + word).length > 55) {
            console.log(`║  ${line.trimEnd().padEnd(55)} ║`);
            line = '    ' + word + ' ';
          } else {
            line += word + ' ';
          }
        }
        if (line.trim()) console.log(`║  ${line.trimEnd().padEnd(55)} ║`);
      });
    }

  console.log(`╚${border}╝\n`);
}

// ─────────────────────────────────────────────────────────────
//  MOCK BEHAVIOURS  (deterministic, no network needed)
// ─────────────────────────────────────────────────────────────
function runMockPass() {
  printResult({
    verdict:  'PASS',
    address:  'MOCK_PASS_ADDRESS_111111111111111111111',
    chain:    'solana (mock)',
    metrics: {
      'Bundler %':       '4 %',
      'Top-10 %':        '22 %',
      'Dev Hold %':      '1.2 %',
      'Liquidity USD':   '$145,000',
      'Liquidity/MC %':  '18 %',
      'Mint Renounced':  'YES',
      'Freeze Renounced':'YES',
      'Honeypot':        'CLEAN',
      'LP Burned':       'YES',
      'Dev Rug History': 'NONE',
      'Wallet Clusters': 'NORMAL',
      'Token Age (s)':   '3,600',
    },
    reasons: [],
  });
}

function runMockFail() {
  printResult({
    verdict:  'SKIP',
    address:  'MOCK_FAIL_ADDRESS_999999999999999999999',
    chain:    'solana (mock)',
    metrics: {
      'Bundler %':       '35 %',
      'Top-10 %':        '67 %',
      'Dev Hold %':      '12 %',
      'Liquidity USD':   '$2,500',
      'Liquidity/MC %':  '3 %',
      'Mint Renounced':  'NO',
      'Freeze Renounced':'NO',
      'Honeypot':        'DETECTED',
      'LP Burned':       'NO',
      'Dev Rug History': '3 prior rugs',
      'Wallet Clusters': 'CLUSTERED',
      'Token Age (s)':   '45',
    },
    reasons: [
      '[Rule 1]  Bundler wallets hold 35 % — exceeds 20 % limit.',
      '[Rule 2]  Top-10 holders control 67 % — exceeds 40 % limit.',
      '[Rule 3]  Dev wallet holds 12 % — exceeds 5 % limit.',
      '[Rule 4]  Liquidity $2,500 — below $10,000 minimum.',
      '[Rule 5]  Liquidity is only 3 % of MC — below 10 % minimum.',
      '[Rule 6]  Mint authority NOT renounced — supply can be inflated.',
      '[Rule 6]  Freeze authority NOT renounced — accounts can be frozen.',
      '[Rule 7]  Honeypot detected — sells may be blocked.',
      '[Rule 8]  LP tokens unburned / unlocked — rug-pull risk.',
      '[Rule 9]  Developer has 3 prior rug-pull events on record.',
      '[Rule 10] Clustered wallet distribution detected.',
      '[Rule 11] Token is only 45 seconds old with excessive market cap.',
    ],
  });
}

// ─────────────────────────────────────────────────────────────
//  GMGN CLI HELPERS  (uses gmgn-cli subprocess — avoids Cloudflare)
// ─────────────────────────────────────────────────────────────
const { execFile } = require('child_process');
const { promisify } = require('util');
const execFileAsync = promisify(execFile);

async function gmgnCli(subcommand, chain, address) {
  // chain is already the correct slug (sol, eth, bsc, base, robinhood, etc.)
  // Only convert the logical 'solana' → 'sol' and 'evm' → 'eth' fallback
  const chainSlug = chain === 'solana' ? 'sol' : chain === 'evm' ? 'eth' : chain;
  try {
    // Use npx so it finds the local node_modules binary when hosted on Railway
    const { stdout } = await execFileAsync('npx', [
      '--no-install',
      'gmgn-cli',
      'token', subcommand,
      '--chain', chainSlug,
      '--address', address,
      '--raw'
    ], {
      timeout: 15_000,
      shell: process.platform === 'win32', // use shell on Windows to resolve npx
      windowsHide: true
    });
    return JSON.parse(stdout.trim());
  } catch (err) {
    throw new Error(`gmgn-cli token ${subcommand} failed: ${err.message}`);
  }
}

// ─────────────────────────────────────────────────────────────
//  DATA FETCHERS
// ─────────────────────────────────────────────────────────────
async function fetchTokenData(address, chain) {
  let resolvedChain = chain;
  if (chain === 'evm') {
    resolvedChain = await detectEvmChain(address);
    console.log(`[chain-detect] EVM address → resolved to: ${resolvedChain}`);
  }

  const slug = resolvedChain === 'solana' ? 'sol' : resolvedChain;
  if (!GMGN_SUPPORTED_CHAINS.has(slug)) {
    console.log(`[GMGN] Chain "${resolvedChain}" not supported — skipping GMGN, using DexScreener only.`);
    return { tokenInfo: {}, security: {}, pool: {}, resolvedChain };
  }

  const [tokenInfo, security, pool] = await Promise.all([
    gmgnCli('info',     resolvedChain, address),
    gmgnCli('security', resolvedChain, address),
    gmgnCli('pool',     resolvedChain, address),
  ]);
  return { tokenInfo, security, pool, resolvedChain };
}

// ─────────────────────────────────────────────────────────────
//  BLOCKSCOUT — On-chain holder analytics + market data for unsupported chains
// ─────────────────────────────────────────────────────────────
async function fetchBlockscoutData(address, chain) {
  const BLOCKSCOUT_URLS = {
    'robinhood': 'https://robinhoodchain.blockscout.com',
    'hype':      'https://hyperliquid.blockscout.com',
  };

  const baseUrl = BLOCKSCOUT_URLS[chain];
  if (!baseUrl) return null;

  const lowercaseAddr = address.toLowerCase();
  const headers = {
    'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)',
    'Accept': 'application/json',
  };

  const result = {
    // Holder data
    top10Pct: null, devHoldPct: null, totalHolders: null, bundlerPct: null, holders: [],
    // Market data (NEW — used as DexScreener fallback)
    name: null, symbol: null, price: 0, marketCap: 0, decimals: 18,
  };

  try {
    const [holdersRes, tokenRes] = await Promise.all([
      axios.get(`${baseUrl}/api/v2/tokens/${lowercaseAddr}/holders?items_count=50`, { headers, timeout: 10_000 }),
      axios.get(`${baseUrl}/api/v2/tokens/${lowercaseAddr}`, { headers, timeout: 8_000 }),
    ]);

    const tokenData = tokenRes.data || {};

    // ── Market data extraction ──
    result.name      = tokenData.name || null;
    result.symbol    = tokenData.symbol || null;
    result.decimals  = Number(tokenData.decimals || 18);
    result.price     = Number(tokenData.exchange_rate || 0);
    result.marketCap = Number(tokenData.circulating_market_cap || 0);

    if (result.name) {
      console.log(`[Blockscout-${chain}] 📊 Token: ${result.name} ($${result.symbol}) | Price: $${result.price} | MC: $${result.marketCap.toLocaleString()}`);
    }

    // ── Holder data extraction ──
    const holderList = holdersRes.data?.items || [];
    const totalSupplyRaw = BigInt(tokenData.total_supply || '0');
    result.totalHolders = tokenData.holders_count ? Number(tokenData.holders_count) : null;

    if (holderList.length > 0 && totalSupplyRaw > 0n) {
      result.holders = holderList.map(h => {
        const addrObj = h.address || {};
        return {
          address: addrObj.hash || addrObj || '',
          pct: Number(BigInt(h.value || '0') * 10000n / totalSupplyRaw) / 100,
          isContract: addrObj.is_contract === true || addrObj.is_smart_contract === true
        };
      });
      // Ignore DEX pools/smart contracts (or any wallet holding >80% which is the curve pool) 
      const realHolders = result.holders.filter(h => !h.isContract && h.pct < 80);
      result.top10Pct = realHolders.slice(0, 10).reduce((s, h) => s + h.pct, 0);

      // ── BUNDLER CLUSTER DETECTION ──
      const clusters = {};
      result.holders.forEach(h => {
        if (h.pct < 0.2) return;
        const key = h.pct.toFixed(2);
        clusters[key] = (clusters[key] || 0) + 1;
      });

      let bundlerTotal = 0;
      for (const [pctStr, count] of Object.entries(clusters)) {
        if (count >= 4) {
          bundlerTotal += Number(pctStr) * count;
          console.log(`[Blockscout-${chain}] 🚨 Bundler Cluster found: ${count} wallets holding exactly ${pctStr}% (Total: ${(Number(pctStr) * count).toFixed(1)}%)`);
        }
      }
      result.bundlerPct = bundlerTotal;
    }

    console.log(`[Blockscout-${chain}] ✅ Top-10: ${result.top10Pct?.toFixed(1) ?? 'N/A'}%  Holders: ${result.totalHolders}  Bundled: ${result.bundlerPct?.toFixed(1) || 0}%`);
  } catch (err) {
    console.warn(`[Blockscout-${chain}] Holder fetch failed: ${err.message}`);
  }

  return result;
}

// ─────────────────────────────────────────────────────────────
//  GOPLUS SECURITY API (Honeypot, Taxes, Freezes)
// ─────────────────────────────────────────────────────────────
const GOPLUS_CHAIN_IDS = {
  'eth': '1',
  'bsc': '56',
  'base': '8453',
  'robinhood': '4663',
  'arbitrum': '42161',
  'optimism': '10',
  'polygon': '137',
  'solana': 'solana'
};

async function fetchGoPlusSecurity(address, chain) {
  const goplusId = GOPLUS_CHAIN_IDS[chain];
  if (!goplusId) return null; // chain not supported by GoPlus yet

  try {
    let url;
    if (goplusId === 'solana') {
      url = `https://api.gopluslabs.io/api/v1/solana/token_security?contract_addresses=${address}`;
    } else {
      url = `https://api.gopluslabs.io/api/v1/token_security/${goplusId}?contract_addresses=${address}`;
    }
    
    const res = await axios.get(url, { timeout: 3500 });
    if (res.data && res.data.result) {
      if (goplusId === 'solana' && res.data.result[address]) {
        return res.data.result[address];
      } else if (res.data.result[address.toLowerCase()]) {
        return res.data.result[address.toLowerCase()];
      }
    }
  } catch (err) {
    console.error(`[GoPlus] Failed to fetch security data: ${err.message}`);
  }
  return null;
}

// ─────────────────────────────────────────────────────────────
//  EVALUATION ENGINE  — 11 hard rules (GMGN / DexScreener)
// ─────────────────────────────────────────────────────────────

/**
 * Evaluate a token. Routes to Loxley for robinhood, GMGN for everything else.
 *
 * @param {string} address
 * @param {string} chain   'solana' | 'evm' | 'robinhood' | ...
 * @returns {Promise<{ verdict:'PASS'|'SKIP', reasons:string[], metrics:{}, resolvedChain:string }>}
 */
async function evaluate(address, chain) {
  const reasons = [];
  const metrics = {};

  // ── 0. Auto-resolve pool addresses to token addresses ─────────
  let dexData = { name: '', symbol: '', liq: 0, mc: 0, vol24: 0, m5Buys: 0, m5Sells: 0, m5Change: 0, chainId: '' };
  try {
    const dexRes = await axios.get(`https://api.dexscreener.com/latest/dex/search?q=${address}`, {
      headers: {
        'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
        'Accept': 'application/json, text/plain, */*',
      },
      timeout: 6000
    });

    // Validate response is actual JSON, not an HTML error page
    if (dexRes.data && typeof dexRes.data === 'object' && dexRes.data.pairs && dexRes.data.pairs.length > 0) {
      let matchingPair = dexRes.data.pairs.find(p => p.pairAddress.toLowerCase() === address.toLowerCase());
      if (!matchingPair) {
        matchingPair = dexRes.data.pairs.sort((a,b) => (b.liquidity?.usd||0) - (a.liquidity?.usd||0))[0];
      }

      if (matchingPair && matchingPair.baseToken) {
        if (address.toLowerCase() !== matchingPair.baseToken.address.toLowerCase()) {
          console.log(`[Auto-Resolve] Converted query '${address}' → Token ${matchingPair.baseToken.address}`);
          address = matchingPair.baseToken.address;
        }
        dexData.chainId = matchingPair.chainId || '';
        dexData.name = matchingPair.baseToken.name || '';
        dexData.symbol = matchingPair.baseToken.symbol || '';
        dexData.liq = Number(matchingPair.liquidity?.usd || 0);
        dexData.mc = Number(matchingPair.marketCap || matchingPair.fdv || 0);
        dexData.vol24 = Number(matchingPair.volume?.h24 || 0);
        dexData.m5Buys = Number(matchingPair.txns?.m5?.buys || 0);
        dexData.m5Sells = Number(matchingPair.txns?.m5?.sells || 0);
        dexData.m5Change = Number(matchingPair.priceChange?.m5 || 0);
        console.log(`[DexScreener] ✅ ${dexData.name} on ${dexData.chainId} — Liq: $${dexData.liq.toLocaleString()} MC: $${dexData.mc.toLocaleString()}`);
      }
    } else {
      console.log(`[DexScreener] ⚠️ No pairs found or blocked — will use Blockscout fallback.`);
    }
  } catch (err) {
    console.warn(`[DexScreener] ⚠️ Fetch failed (${err.message}) — will use Blockscout fallback.`);
  }

  // ── Resolve chain ─────────────────────────────────────────────
  //  Priority: manual override → DexScreener chainId → Blockscout probe → GMGN race → eth fallback
  let resolvedChain = chain;
  if (chain === 'evm') {
    if (dexData.chainId) {
      const cid = dexData.chainId.toLowerCase();
      if (cid === 'ethereum') resolvedChain = 'eth';
      else if (cid === 'hyperliquid' || cid === 'hyperevm') resolvedChain = 'hype';
      else resolvedChain = cid;
      console.log(`[chain-detect] DexScreener → ${resolvedChain}`);
    } else {
      resolvedChain = await detectEvmChain(address);
      console.log(`[chain-detect] Fallback → ${resolvedChain}`);
    }
  }
  // If chain was set directly by manual override (e.g. 'robinhood'), it stays as-is.

  // ── Fire GMGN + Blockscout + GoPlus in parallel ──────────────
  const [raw, blockscoutData, goPlusData] = await Promise.all([
    fetchTokenData(address, resolvedChain).catch(err => {
      console.warn(`[GMGN] Failed (${err.message}). Using DexScreener/Blockscout.`);
      return {};
    }),
    fetchBlockscoutData(address, resolvedChain),
    fetchGoPlusSecurity(address, resolvedChain)
  ]);

  resolvedChain = raw.resolvedChain || resolvedChain;

  const tInfo  = raw.tokenInfo  || {};
  const tSec   = raw.security   || {};
  const stat   = tInfo.stat  || {};
  const price  = tInfo.price || {};
  const dev    = tInfo.dev   || {};

  // ── Metric extraction ──────────────────────────────────────
  //  Priority: GMGN → DexScreener → Blockscout
  const tokenSymbol = tInfo.token?.symbol || tInfo.symbol || dexData.symbol || blockscoutData?.symbol || 'UNKNOWN';
  const tokenName   = tInfo.token?.name || tInfo.name || dexData.name || blockscoutData?.name || '';
  
  const currentPrice = Number(price.price || 0);
  const circSupply   = Number(tInfo.circulating_supply || 0);

  // Market Cap: DexScreener → GMGN → Blockscout
  let marketCapUSD = dexData.mc > 0 ? dexData.mc
    : ((currentPrice * circSupply > 0) ? (currentPrice * circSupply) : 0);
  if (marketCapUSD === 0 && blockscoutData?.marketCap > 0) {
    marketCapUSD = blockscoutData.marketCap;
    console.log(`[Blockscout] 📊 Using Blockscout market cap: $${marketCapUSD.toLocaleString()}`);
  }

  // Liquidity: DexScreener → GMGN → (no Blockscout equivalent, but curve chains skip liq rules)
  const liquidityUSD = dexData.liq > 0 ? dexData.liq : Number(tInfo.liquidity || 0);

  // ── Holder data: prefer Blockscout (real on-chain) over GMGN for unsupported chains ──
  let hasHolderData, top10Pct, devHoldPct, bundlerPct;
  if (blockscoutData && blockscoutData.top10Pct !== null) {
    hasHolderData = true;
    top10Pct      = blockscoutData.top10Pct;
    devHoldPct    = blockscoutData.devHoldPct ?? (stat.creator_hold_rate !== undefined ? Number(stat.creator_hold_rate) * 100 : 0);
    bundlerPct    = blockscoutData.bundlerPct ?? (stat.top_bundler_trader_percentage !== undefined ? Number(stat.top_bundler_trader_percentage) * 100 : 0);
    console.log(`[Blockscout-${resolvedChain}] ✅ Using real on-chain holder data — Top10: ${top10Pct.toFixed(1)}%`);
  } else if (resolvedChain === 'solana' && goPlusData && goPlusData.holders && goPlusData.holders.length > 0) {
    // If GMGN fails on Solana, calculate Top 10 directly from GoPlus security holders array
    hasHolderData = true;
    top10Pct = goPlusData.holders.slice(0, 10).reduce((sum, h) => sum + (Number(h.percent || 0) * 100), 0);
    devHoldPct = stat.creator_hold_rate !== undefined ? Number(stat.creator_hold_rate) * 100 : 0;
    bundlerPct = stat.top_bundler_trader_percentage !== undefined ? Number(stat.top_bundler_trader_percentage) * 100 : 0;
    console.log(`[GoPlus-Solana] ✅ Using GoPlus holder data — Top10: ${top10Pct.toFixed(1)}%`);
  } else {
    hasHolderData = stat.top_10_holder_rate !== undefined;
    top10Pct      = hasHolderData ? Number(stat.top_10_holder_rate) * 100 : 0;
    devHoldPct    = stat.creator_hold_rate !== undefined ? Number(stat.creator_hold_rate) * 100 : 0;
    bundlerPct    = stat.top_bundler_trader_percentage !== undefined ? Number(stat.top_bundler_trader_percentage) * 100 : 0;
  }

  const mintAuth = resolvedChain === 'solana'
    ? (goPlusData ? goPlusData.mintable?.status === '0' : tSec.renounced_mint === true)
    : (goPlusData ? goPlusData.cannot_mint === '1' : tSec.owner_renounced === 'yes');
    
  const freezeAuth = resolvedChain === 'solana'
    ? (goPlusData ? goPlusData.freezable?.status === '0' : tSec.renounced_freeze_account === true)
    : true; // EVM has no freeze authority concept
    
  const isHoneypot = goPlusData
    ? (resolvedChain === 'solana' ? false : goPlusData.is_honeypot === '1')
    : (tSec.is_honeypot === 'yes');
  const lpBurned   = tSec.burn_status === 'burn';
  const lpLocked   = Number(tInfo.locked_ratio || 0) > 0;

  // ── Dev / Rug signals from real GMGN fields ──────────────────
  const rugRatio      = Number(tSec.rug_ratio || 0);
  const isWashTrading = tSec.is_wash_trading === true;
  const sniperCount   = Number(tSec.sniper_count || 0);
  const devLaunchCount = Number(dev.creator_open_count || 0);
  const devTokenStatus = tSec.creator_token_status || dev.creator_token_status || '';
  const devStillHolding = devTokenStatus === 'creator_hold';

  const clustered   = false;
  const tokenAgeSec = tInfo.creation_timestamp ? ((Date.now() / 1000) - Number(tInfo.creation_timestamp)) : 0;

  const liqMcPct = marketCapUSD > 0 ? (liquidityUSD / marketCapUSD) * 100 : 0;
  const vol24 = Number(tInfo.volume_24h || dexData.vol24 || 0);
  const turnover = liquidityUSD > 0 ? (vol24 / liquidityUSD) : 0;

  const isCurveChain = (resolvedChain === 'robinhood' || resolvedChain === 'hype');

  // ── Smart Scoring Engine (Safety & Momentum) ───────────────
  const getGrade = (s) => (s >= 90 ? 'A+' : s >= 80 ? 'A' : s >= 70 ? 'B' : s >= 60 ? 'C' : s >= 50 ? 'D' : 'F');

  let safetyScore = 100;
  const sLog = [];
  if (bundlerPct > 0) { const d = Math.min(30, bundlerPct * 1.5); safetyScore -= d; sLog.push(`-${Math.round(d)} Sniper/Bundlers`); }
  if (top10Pct > 10) { const d = Math.min(40, (top10Pct - 10)); safetyScore -= d; sLog.push(`-${Math.round(d)} Top10 Cartel`); }
  if (devHoldPct > 0) { const d = Math.min(30, devHoldPct * 3); safetyScore -= d; sLog.push(`-${Math.round(d)} Dev Holds`); }
  if (isHoneypot) { safetyScore -= 100; sLog.push('-100 Honeypot'); }
  if (!isCurveChain && !lpBurned && !lpLocked) { safetyScore -= 30; sLog.push('-30 LP Unlocked'); } else if (lpBurned) { sLog.push('+0 LP Burned'); }
  if (!isCurveChain && liquidityUSD < 10000) { safetyScore -= 20; sLog.push('-20 Low Liquidity'); }
  if (rugRatio > 0.3) { safetyScore -= 40; sLog.push(`-40 High Rug Risk (${(rugRatio*100).toFixed(0)}%)`); }
  else if (rugRatio > 0.1) { safetyScore -= 15; sLog.push(`-15 Med Rug Risk (${(rugRatio*100).toFixed(0)}%)`); }
  if (devLaunchCount > 10) { safetyScore -= 20; sLog.push(`-20 Serial Deployer (${devLaunchCount} tokens)`); }
  if (devStillHolding) { safetyScore -= 10; sLog.push('-10 Dev Holding'); }
  if (sniperCount > 20) { safetyScore -= 20; sLog.push(`-20 Heavy Snipers (${sniperCount})`); }
  else if (sniperCount > 5) { safetyScore -= 10; sLog.push(`-10 Snipers (${sniperCount})`); }
  safetyScore = Math.max(0, Math.round(safetyScore));

  let momentumScore = 50;
  const mLog = [];
  const totalM5Tx = dexData.m5Buys + dexData.m5Sells;
  if (totalM5Tx > 10) {
    const buyRatio = dexData.m5Buys / totalM5Tx;
    if (buyRatio > 0.6) { momentumScore += 20; mLog.push(`+20 High Buys (${Math.round(buyRatio*100)}%)`); }
    else if (buyRatio < 0.4) { momentumScore -= 20; mLog.push(`-20 High Sells (${Math.round(buyRatio*100)}%)`); }
  }
  if (dexData.m5Change > 10) { momentumScore += 20; mLog.push(`+20 Pumping (+${dexData.m5Change}%)`); }
  else if (dexData.m5Change < -10) { momentumScore -= 20; mLog.push(`-20 Dumping (${dexData.m5Change}%)`); }
  if (isWashTrading || turnover > 40) { momentumScore -= 40; mLog.push(`-40 Wash Trading`); }
  momentumScore = Math.max(0, Math.min(100, Math.round(momentumScore)));

  const conviction = Math.round((safetyScore + momentumScore) / 2);

  const tags = [];
  if (liquidityUSD > 50000) tags.push('💧 Deep Pool');
  else if (liquidityUSD > 0 && liquidityUSD < 10000) tags.push('🏜️ Thin Liq');
  if (top10Pct > 30) tags.push('🐋 Whale Heavy');
  if (lpBurned) tags.push('🔥 LP Burned');
  if (isWashTrading || turnover > 40) tags.push('🤖 Wash Traded');
  if (dexData.m5Buys > dexData.m5Sells * 2 && totalM5Tx > 15) tags.push('🚀 Buy Pressure');
  if (!hasHolderData) tags.push('🙈 No Holder Data');
  if (rugRatio > 0.3) tags.push('🚨 High Rug Risk');
  if (devLaunchCount > 10) tags.push('⚠️ Serial Deployer');
  if (sniperCount > 20) tags.push('🎯 Heavy Sniped');

  // ── Populate metrics display block ──────────────────────────
  metrics['Token']            = `${tokenName} ($${tokenSymbol})`.trim();
  metrics['Conviction']       = `${conviction}/100`;
  metrics['Grades']           = `🛡️ Safety ${getGrade(safetyScore)} | ⚡ Momentum ${getGrade(momentumScore)}`;
  if (tags.length > 0) metrics['Tags'] = tags.join(' ');
  metrics['Breakdown']        = `[Safety] ${sLog.join(', ')}  [Momentum] ${mLog.join(', ')}`;
  
  // Add Bubblemaps link
  const bubblemapsSlugs = {
    'eth': 'eth', 'bsc': 'bsc', 'base': 'base', 'sol': 'sol',
    'robinhood': 'robinhood', 'hype': 'hyperevm', 'ink': 'ink'
  };
  if (bubblemapsSlugs[resolvedChain]) {
    metrics['Bubblemaps'] = `https://app.bubblemaps.io/token/${bubblemapsSlugs[resolvedChain]}/${address}`;
  }

  metrics['Bundler %']        = hasHolderData ? `${bundlerPct.toFixed(1)} %` : 'N/A';
  metrics['Top-10 %']         = hasHolderData ? `${top10Pct.toFixed(1)} %` : 'N/A';
  metrics['Dev Hold %']       = hasHolderData ? `${devHoldPct.toFixed(1)} %` : 'N/A';
  metrics['Liquidity USD']    = `$${liquidityUSD.toLocaleString()}`;
  metrics['Market Cap USD']   = `$${marketCapUSD.toLocaleString()}`;
  metrics['Liquidity/MC %']   = `${liqMcPct.toFixed(1)} %`;
  metrics['Contract Owner']   = mintAuth ? 'RENOUNCED' : 'NOT RENOUNCED';
  if (resolvedChain === 'solana') metrics['Freeze Auth'] = freezeAuth ? 'RENOUNCED' : 'NOT RENOUNCED';
  metrics['Honeypot']         = isHoneypot ? 'DETECTED' : 'CLEAN';
  metrics['LP Burned']        = lpBurned ? 'YES' : (lpLocked ? 'LOCKED' : 'NO');
  metrics['Rug Risk Score']   = rugRatio > 0 ? `${(rugRatio * 100).toFixed(0)}% ${rugRatio > 0.3 ? '🚨' : rugRatio > 0.1 ? '⚠️' : '✅'}` : 'N/A';
  metrics['Sniper Count']     = sniperCount > 0 ? `${sniperCount} ${sniperCount > 20 ? '🚨' : sniperCount > 5 ? '⚠️' : '✅'}` : '0 ✅';
  metrics['Dev Launches']     = devLaunchCount > 0 ? `${devLaunchCount} tokens ${devLaunchCount > 10 ? '⚠️' : ''}` : 'N/A';
  metrics['Dev Status']       = devTokenStatus ? (devStillHolding ? '⚠️ Still Holding' : '✅ Sold/Exited') : 'N/A';
  if (tokenAgeSec > 0) metrics['Token Age'] = formatAge(tokenAgeSec);

  let isSkip = false;
  let isGamble = false;

  // ══ GOPLUS SECURITY DEEP SCAN ════════════════════════════════
  if (goPlusData) {
    if (resolvedChain === 'solana') {
      if (goPlusData.mintable?.status === '1') {
         isSkip = true; reasons.push(`[GoPlus] Token is MINTABLE (Dev can print infinite tokens).`);
      }
      if (goPlusData.freezable?.status === '1') {
         isSkip = true; reasons.push(`[GoPlus] Token is FREEZABLE (Dev can freeze your wallet).`);
      }
      if (goPlusData.transfer_fee_upgradable?.status === '1') {
         isGamble = true; reasons.push(`[GoPlus] Transfer Tax can be modified by the dev.`);
      }
    } else {
      if (goPlusData.is_honeypot === '1') {
         isSkip = true; reasons.push(`[GoPlus] HARD HONEYPOT DETECTED! Do not buy.`);
      }
      if (goPlusData.is_mintable === '1') {
         isSkip = true; reasons.push(`[GoPlus] Token is MINTABLE (Dev can print infinite tokens).`);
      }
      if (goPlusData.transfer_pausable === '1') {
         isSkip = true; reasons.push(`[GoPlus] Transfers can be PAUSED by the dev (Trap Risk).`);
      }
      if (goPlusData.is_blacklisted === '1' || goPlusData.is_whitelisted === '1') {
         isSkip = true; reasons.push(`[GoPlus] Blacklist/Whitelist functions detected (Censorship Risk).`);
      }
      if (Number(goPlusData.sell_tax || 0) > 0.1) {
         isSkip = true; reasons.push(`[GoPlus] High Sell Tax Detected: ${(Number(goPlusData.sell_tax)*100).toFixed(1)}%.`);
      } else if (Number(goPlusData.sell_tax || 0) > 0.05) {
         isGamble = true; reasons.push(`[GoPlus] Moderate Sell Tax Detected: ${(Number(goPlusData.sell_tax)*100).toFixed(1)}%.`);
      }
      if (goPlusData.slippage_modifiable === '1' || goPlusData.personal_slippage_modifiable === '1') {
         isGamble = true; reasons.push(`[GoPlus] Dev can modify taxes/slippage at any time.`);
      }
    }
  }

  // ══ RULE 1 — Bundler concentration ═══════════════════════════
  if (bundlerPct > LOOSE_RULE.BUNDLER_MAX_PCT) {
    isSkip = true;
    reasons.push(`[Rule 1]  Bundler wallets hold ${bundlerPct.toFixed(1)} % — exceeds loose limit (${LOOSE_RULE.BUNDLER_MAX_PCT}%).`);
  } else if (bundlerPct > RULE.BUNDLER_MAX_PCT) {
    isGamble = true;
    reasons.push(`[Rule 1]  Bundler wallets hold ${bundlerPct.toFixed(1)} % — exceeds strict limit (${RULE.BUNDLER_MAX_PCT}%).`);
  }

  // ══ RULE 2 — Top-10 wallets (only if we have real holder data) ═
  if (hasHolderData && top10Pct > LOOSE_RULE.TOP10_MAX_PCT) {
    isSkip = true;
    reasons.push(`[Rule 2]  Top-10 holders control ${top10Pct.toFixed(1)} % — exceeds loose limit (${LOOSE_RULE.TOP10_MAX_PCT}%).`);
  } else if (hasHolderData && top10Pct > RULE.TOP10_MAX_PCT) {
    isGamble = true;
    reasons.push(`[Rule 2]  Top-10 holders control ${top10Pct.toFixed(1)} % — exceeds strict limit (${RULE.TOP10_MAX_PCT}%).`);
  }

  // ══ RULE 3 — Dev wallet holds ════════════════════════════════
  if (devHoldPct > LOOSE_RULE.DEV_HOLD_MAX_PCT) {
    isSkip = true;
    reasons.push(`[Rule 3]  Dev wallet holds ${devHoldPct.toFixed(1)} % — exceeds loose limit (${LOOSE_RULE.DEV_HOLD_MAX_PCT}%).`);
  } else if (devHoldPct > RULE.DEV_HOLD_MAX_PCT) {
    isGamble = true;
    reasons.push(`[Rule 3]  Dev wallet holds ${devHoldPct.toFixed(1)} % — exceeds strict limit (${RULE.DEV_HOLD_MAX_PCT}%).`);
  }

  // ══ RULE 4 — Liquidity (minimum) ═════════════════════════════
  if (!isCurveChain) {
    if (liquidityUSD < LOOSE_RULE.LIQ_MIN_USD) {
      isSkip = true;
      reasons.push(`[Rule 4]  Liquidity $${liquidityUSD.toLocaleString()} — below loose minimum ($${LOOSE_RULE.LIQ_MIN_USD.toLocaleString()}).`);
    } else if (liquidityUSD < RULE.LIQ_MIN_USD) {
      isGamble = true;
      reasons.push(`[Rule 4]  Liquidity $${liquidityUSD.toLocaleString()} — below strict minimum ($${RULE.LIQ_MIN_USD.toLocaleString()}).`);
    }
  }

  // ══ RULE 5 — Liquidity / MC Ratio ════════════════════════════
  if (!isCurveChain) {
    if (liqMcPct < LOOSE_RULE.LIQ_MIN_MC_PCT) {
      isSkip = true;
      reasons.push(`[Rule 5]  Liquidity is ${liqMcPct.toFixed(1)} % of MC — below loose minimum (${LOOSE_RULE.LIQ_MIN_MC_PCT}%).`);
    } else if (liqMcPct < RULE.LIQ_MIN_MC_PCT) {
      isGamble = true;
      reasons.push(`[Rule 5]  Liquidity is ${liqMcPct.toFixed(1)} % of MC — below strict minimum (${RULE.LIQ_MIN_MC_PCT}%).`);
    }
  }

  // ══ HARD BOOLEAN RULES (Always SKIP if failed) ═══════════════
  if (!mintAuth && !isCurveChain) {
    isSkip = true;
    reasons.push('[Rule 6]  Mint authority NOT renounced — supply can be inflated at will.');
  }
  if (!freezeAuth && !isCurveChain) {
    isSkip = true;
    reasons.push('[Rule 6]  Freeze authority NOT renounced — token accounts can be frozen.');
  }
  if (isHoneypot) {
    isSkip = true;
    reasons.push('[Rule 7]  Honeypot detected — sell transactions may be blocked.');
  }
  if (!lpBurned && !lpLocked && !isCurveChain) {
    if (resolvedChain === 'solana' && Object.keys(tSec).length === 0) {
      isGamble = true;
      reasons.push('[Rule 8]  LP status unknown (missing API data) — proceed with extreme caution.');
    } else {
      isSkip = true;
      reasons.push('[Rule 8]  LP tokens are unburned and unlocked — rug-pull risk is high.');
    }
  }
  if (rugRatio > 0.3) {
    isSkip = true;
    reasons.push(`[Rule 9]  Rug risk score is ${(rugRatio*100).toFixed(0)}% — GMGN flags this as high-risk.`);
  } else if (rugRatio > 0.1) {
    isGamble = true;
    reasons.push(`[Rule 9]  Rug risk score is ${(rugRatio*100).toFixed(0)}% — medium risk, proceed carefully.`);
  }
  if (clustered) {
    isSkip = true;
    reasons.push('[Rule 10] Clustered wallet distribution detected — coordinated dump risk.');
  }
  if (tokenAgeSec > 0 && tokenAgeSec < 120 && marketCapUSD > 5_000_000) {
    isSkip = true;
    reasons.push('[Rule 11] Token is under 2 minutes old but has excessive market cap (>$5M).');
  }
  if ((isWashTrading || turnover > 40) && !isCurveChain) {
    isSkip = true;
    reasons.push(`[Rule 12] Wash Trading detected — 24h volume is ${turnover.toFixed(1)}x the pool liquidity.`);
  } else if (turnover > 200 && isCurveChain) {
    isSkip = true;
    reasons.push(`[Rule 12] Extreme Wash Trading detected — 24h volume is ${turnover.toFixed(1)}x the curve pool.`);
  }
  if (devLaunchCount > 10) {
    isGamble = true;
    reasons.push(`[Rule 13] Serial deployer — this dev has launched ${devLaunchCount} tokens before.`);
  }

  const verdict = isSkip ? 'SKIP' : (isGamble ? 'GAMBLE' : 'BUY');
  return { verdict, reasons, metrics, resolvedChain };
}

// ─────────────────────────────────────────────────────────────
//  STAGE-2 STUBS  (future automation hooks)
// ─────────────────────────────────────────────────────────────

/**
 * Stage 2 — Watch a Telegram channel for new calls and pipe each
 * token address through the evaluation engine automatically.
 * Requires Telegram Bot API token in environment (TELEGRAM_BOT_TOKEN).
 *
 * @param {string} channelUsername
 */
async function watchTelegram(channelUsername) {
  const token = process.env.TELEGRAM_BOT_TOKEN;
  if (!token) {
    console.error('[ERROR] TELEGRAM_BOT_TOKEN is missing in .env');
    return;
  }
  
  // Requires: npm install node-telegram-bot-api
  const TelegramBot = require('node-telegram-bot-api');
  const bot = new TelegramBot(token, { polling: true });

  console.log(`\n[Telegram] Bot started. Listening for messages...`);
  if (channelUsername) console.log(`[Telegram] Filtering for channel: ${channelUsername}`);
  
  bot.on('message', async (msg) => {
    if (channelUsername && msg.chat.username !== channelUsername.replace('@', '')) return;

    const text = msg.text || msg.caption || '';
    if (!text) return;

    const { address, chain } = extractAddress(text);
    if (address) {
      console.log(`\n[Telegram] Detected ${chain} address: ${address}`);
      try {
        const result = await evaluate(address, chain);
        printResult({ ...result, address, chain });
        
        if (result.verdict === 'PASS') {
          console.log(`[Telegram] Token PASSED evaluation. Triggering test buy...`);
          await buySmall(address, chain, 5); // Default $5 buy
        }
      } catch (err) {
        console.error(`[Telegram] Evaluation failed:`, err.message);
      }
    }
  });
}

/**
 * Stage 2 — Execute a small test buy on a passing token via Jupiter
 * to establish a position with minimal risk.
 * Requires SOLANA_PRIVATE_KEY in environment for signing.
 *
 * @param {string} address
 * @param {string} chain
 * @param {number} usdAmount
 */
async function buySmall(address, chain, usdAmount) {
  if (chain !== 'solana') {
    console.log(`[Buy] Only Solana buys are supported (got ${chain}). Skipping.`);
    return;
  }

  const privateKeyBase58 = process.env.SOLANA_PRIVATE_KEY;
  if (!privateKeyBase58) {
    console.log('[Buy] SOLANA_PRIVATE_KEY not set in .env — skipping buy.');
    return;
  }

  // Requires: npm install @solana/web3.js bs58
  let web3, bs58;
  try {
    web3 = require('@solana/web3.js');
    bs58 = require('bs58');
  } catch {
    console.error('[Buy] Missing deps — run: npm install @solana/web3.js bs58');
    return;
  }

  try {
    // ── 1. Load wallet ──────────────────────────────────────
    const secretKey = bs58.decode(privateKeyBase58);
    const keypair   = web3.Keypair.fromSecretKey(secretKey);
    console.log(`[Buy] Wallet: ${keypair.publicKey.toBase58()}`);

    // ── 2. Get Jupiter quote ─────────────────────────────────
    const USDC_MINT   = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
    const inputAmount = Math.round(usdAmount * 1_000_000); // USDC has 6 decimals

    console.log(`[Buy] Fetching Jupiter quote: $${usdAmount} USDC → ${address}`);
    const quoteRes = await axios.get('https://quote-api.jup.ag/v6/quote', {
      params: {
        inputMint:    USDC_MINT,
        outputMint:   address,
        amount:       inputAmount,
        slippageBps:  300,  // 3%
      }
    });
    const quote = quoteRes.data;
    console.log(`[Buy] Quote: ${quote.outAmount} tokens out`);

    // ── 3. Get swap transaction from Jupiter ─────────────────
    const swapRes = await axios.post('https://quote-api.jup.ag/v6/swap', {
      quoteResponse:        quote,
      userPublicKey:        keypair.publicKey.toBase58(),
      wrapAndUnwrapSol:     true,
      dynamicComputeUnitLimit: true,
      prioritizationFeeLamports: 'auto',
    });
    const { swapTransaction } = swapRes.data;

    // ── 4. Deserialize, sign, and send ───────────────────────
    const connection = new web3.Connection(
      process.env.SOLANA_RPC_URL || 'https://api.mainnet-beta.solana.com',
      'confirmed'
    );

    const txBuf  = Buffer.from(swapTransaction, 'base64');
    const tx     = web3.VersionedTransaction.deserialize(txBuf);
    tx.sign([keypair]);

    const sig = await connection.sendRawTransaction(tx.serialize(), {
      skipPreflight: false,
      maxRetries:    3,
    });

    console.log(`[Buy] ✅ Swap sent! Signature: ${sig}`);
    console.log(`[Buy] Explorer: https://solscan.io/tx/${sig}`);
    return sig;

  } catch (err) {
    const detail = err.response ? JSON.stringify(err.response.data).substring(0, 200) : err.message;
    console.error(`[Buy] Failed: ${detail}`);
  }
}

// ─────────────────────────────────────────────────────────────
//  MAIN  — CLI entrypoint
// ─────────────────────────────────────────────────────────────
async function main() {
  const rawInput = process.argv.slice(2).join(' ').trim();

  if (!rawInput) {
    console.error('\nUsage: node gate.js <token_address_or_text>\n');
    console.error('  Examples:');
    console.error('    node gate.js 7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU');
    console.error('    node gate.js "Check this gem! 0xabc...def pumping rn"');
    console.error('    node gate.js MOCK_PASS');
    console.error('    node gate.js MOCK_FAIL\n');
    process.exit(1);
  }

  // ── Deterministic mock modes ─────────────────────────────────
  if (rawInput === 'MOCK_PASS') { runMockPass(); return; }
  if (rawInput === 'MOCK_FAIL') { runMockFail(); return; }

  // ── Extract address ──────────────────────────────────────────
  const { address, chain } = extractAddress(rawInput);

  if (!address) {
    console.error('\n[ERROR] No valid Solana (Base58) or EVM (0x…) address found in input.\n');
    process.exit(1);
  }

  // ── Require API key for live evaluation ─────────────────────
  if (!GMGN_API_KEY) {
    console.error('\n[ERROR] GMGN_API_KEY is not set. Copy .env.example → .env and add your key.\n');
    process.exit(1);
  }

  console.log(`\n[call-gate] Evaluating ${chain} token: ${address} …`);

  // ── Run evaluation ───────────────────────────────────────────
  let result;
  try {
    result = await evaluate(address, chain);
  } catch (err) {
    // Unhandled evaluation error — default to SKIP
    result = {
      verdict:  'SKIP',
      address,
      chain,
      metrics:  {},
      reasons:  [`[FATAL] Unhandled evaluation error: ${err.message}`],
    };
  }

  printResult({ ...result, address, chain });

  // Exit code: 0 = PASS, 1 = SKIP/error
  process.exit(result.verdict === 'PASS' ? 0 : 1);
}

if (require.main === module) {
  main().catch(err => {
    console.error('\n[FATAL]', err.message);
    process.exit(1);
  });
}

module.exports = {
  evaluate,
  extractAddress,
  printResult,
  runMockPass,
  runMockFail
};
