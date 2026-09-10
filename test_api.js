const https = require('https');

https.get('https://explorer.inkonchain.com/api', (res) => {
  console.log('Ink:', res.statusCode);
});

https.get('https://purrsec.com/api', (res) => {
  console.log('Purrsec:', res.statusCode);
});
