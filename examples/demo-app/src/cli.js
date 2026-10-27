import { add, clamp, average } from "./math.js";

const values = process.argv.slice(2).map(Number);
if (values.length === 0) {
  console.log("usage: npm start -- 1 2 3");
  process.exit(0);
}
console.log(`sum: ${values.reduce((acc, value) => add(acc, value), 0)}`);
console.log(`average: ${average(values)}`);
console.log(`clamped: ${clamp(values[0], 0, 10)}`);
