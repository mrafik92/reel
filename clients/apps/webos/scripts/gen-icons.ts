import { writeIcon } from "./icons";
import { resolve } from "node:path";
const dir = resolve(import.meta.dir, "../assets");
writeIcon(`${dir}/icon.png`, 80, 80);
writeIcon(`${dir}/largeIcon.png`, 130, 130);
writeIcon(`${dir}/splash.png`, 1920, 1080);
console.log("wrote", dir);
