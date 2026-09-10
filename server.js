require('dotenv').config();
const express = require('express');
const path    = require('path');
const TelegramBot = require('node-telegram-bot-api');
const { evaluate, extractAddress } = require('./gate.js');

const app = express();
app.use(express.json());
app.use(express.static(path.join(__dirname, 'public'))); // serve website

// ─────────────────────────────────────────────────────────────
//  SHARED HANDLER  — evaluate + reply (used by both DMs & channels)
// ─────────────────────────────────────────────────────────────
async function handleTgMessage(bot, msg) {
  const chatId = msg.chat.id;
  const text   = msg.text || msg.caption || '';
  if (!text) return;

  const { address, chain } = extractAddress(text);
  if (!address) {
    // Only complain in DMs — stay quiet in channels
    if (msg.chat.type === 'private') {
      bot.sendMessage(chatId, '⚠️ Please send a valid Solana or EVM token address.');
    }
    return;
  }

  bot.sendMessage(chatId,
    `⏳ Scanning ${chain} token: \`${address}\`…`,
    { parse_mode: 'Markdown' }
  );

  try {
    const result = await evaluate(address, chain);
    const displayChain = result.resolvedChain || chain;
    const pass = result.verdict === 'BUY' || result.verdict === 'GAMBLE';
    const vtext = result.verdict === 'BUY' ? '🟢 BUY' : result.verdict === 'GAMBLE' ? '🟡 GAMBLE' : '🚫 SKIP';

    let reply = `*CALL-GATE: ${vtext}*\n`;
    reply += `Chain: \`${displayChain}\`  |  Address: \`${address}\`\n\n`;

    if (result.metrics) {
      reply += `*Metrics:*\n`;
      for (const [k, v] of Object.entries(result.metrics)) {
        reply += `• ${k}: ${v}\n`;
      }
    }

    if (!pass && result.reasons.length) {
      reply += `\n*Why:*\n`;
      result.reasons.forEach(r => {
        reply += `❌ ${r.replace(/[_*`[\]]/g, '')}\n`;
      });
    }

    // ── Quick Buy Buttons ────────────────────────────────────
    const inlineKeyboard = [];
    const checkChain = (displayChain || '').toLowerCase();
    
    if (checkChain === 'sol' || checkChain === 'solana') {
      inlineKeyboard.push([{ text: '🐴 Buy with Trojan', url: `https://t.me/solana_trojan_bot?start=r-memescan-${address}` }]);
    } else {
      inlineKeyboard.push([{ text: '🔵 Buy with BasedBot', url: `https://t.me/based_eth_bot?start=b_${address}` }]);
    }

    const sendOptions = { 
      parse_mode: 'Markdown', 
      disable_web_page_preview: true 
    };

    if (inlineKeyboard.length > 0) {
      sendOptions.reply_markup = { inline_keyboard: inlineKeyboard };
    }

    if (reply.length > 4000) reply = reply.substring(0, 3990) + '\n_(truncated)_';
    bot.sendMessage(chatId, reply, sendOptions);
  } catch (err) {
    bot.sendMessage(chatId, `❌ Evaluation error: ${err.message}`);
  }
}

// ─────────────────────────────────────────────────────────────
//  TELEGRAM BOT
// ─────────────────────────────────────────────────────────────
const tgToken = process.env.TELEGRAM_BOT_TOKEN;
if (tgToken) {
  const bot = new TelegramBot(tgToken, { polling: true });
  console.log('[Telegram] Bot is running…');

  // DM / group messages
  bot.on('message', (msg) => handleTgMessage(bot, msg));

  // Channel posts — bot must be admin in the channel
  bot.on('channel_post', (msg) => handleTgMessage(bot, msg));

} else {
  console.warn('[Telegram] TELEGRAM_BOT_TOKEN not set — bot disabled.');
}

// ─────────────────────────────────────────────────────────────
//  API
// ─────────────────────────────────────────────────────────────
app.post('/api/evaluate', async (req, res) => {
  const { input } = req.body;
  if (!input) return res.status(400).json({ error: 'Missing input' });

  const { address, chain } = extractAddress(input);
  if (!address) return res.status(400).json({ error: 'No valid address found' });

  try {
    const result = await evaluate(address, chain);
    res.json({ address, chain, ...result });
  } catch (err) {
    res.status(500).json({ error: err.message });
  }
});

const PORT = process.env.PORT || 3000;
app.listen(PORT, () => {
  console.log(`[Web] API and Website running on http://localhost:${PORT}`);
});
