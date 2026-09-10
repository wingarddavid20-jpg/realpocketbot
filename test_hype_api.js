const axios = require('axios');

async function test() {
  const addr = '0xf09703969cf55aa8a05ee9c76ab3013477283666'; // SIGNAL
  const urls = [
    `https://purrsec.com/api/v2/tokens/${addr}`,
    `https://purrsec.com/api?module=token&action=getTokenHolders&contractaddress=${addr}&page=1&offset=50`,
    `https://hyperevmscan.io/api/v2/tokens/${addr}`,
    `https://explorer.hyperliquid.xyz/api/v2/tokens/${addr}`
  ];

  for (const url of urls) {
    try {
      const res = await axios.get(url, { timeout: 5000 });
      console.log(`[SUCCESS] ${url} -> status ${res.status}, keys: ${Object.keys(res.data).join(',')}`);
    } catch (e) {
      console.log(`[ERROR] ${url} -> ${e.response ? e.response.status : e.message}`);
    }
  }
}
test();
