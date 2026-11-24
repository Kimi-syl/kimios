const fs = require("fs");
const out = process.env.HOME + "/smoke-js.txt";
fs.writeFileSync(out, "js-ok version=" + process.version +
  " platform=" + process.platform + " arch=" + process.arch + "\n");