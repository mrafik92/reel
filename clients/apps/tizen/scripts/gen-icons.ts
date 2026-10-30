import { writeIcon } from "../../webos/scripts/icons";
import { resolve } from "node:path";
const dir = resolve(import.meta.dir, "../assets");
writeIcon(`${dir}/icon.png`, 512, 512);
console.log("wrote", dir);
