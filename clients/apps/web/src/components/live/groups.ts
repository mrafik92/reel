/**
 * Group classification for big IPTV providers (150+ "buckets" with names like
 * "AR: beIN Sports", "Shahid Sports | شاهد سبورت", "UK| Movies", "Africa - أفريقيا").
 *
 * Two independent facets per group: a TOPIC (sports, movies…) and a REGION (Arabic, UK…).
 * Both come from keyword + prefix tables below — extend them freely; first match wins, so put
 * specific entries before generic ones.
 */

export interface Facet {
  id: string;
  label: string;
  /** Matched as whole words / word starts in the folded name (Latin and Arabic). */
  keywords: string[];
  /** Upper-case codes matched as a leading tag: "AR:", "UK|", "FR -", "[DE]". */
  prefixes?: string[];
}

export const TOPICS: Facet[] = [
  {
    id: "sports",
    label: "Sports",
    keywords: [
      "sport", "sports", "bein", "espn", "dazn", "ssc", "alkass", "al kass", "arena", "football", "soccer", "nba", "nfl", "f1",
      "racing", "tennis", "golf", "fight", "boxing", "ufc", "wwe", "match", "eurosport", "supersport", "sky sport", "premier league",
      "رياضه", "رياضيه", "رياضة", "رياضية", "سبورت", "سبورتس", "الكاس", "كره", "كرة", "دوري",
    ],
  },
  {
    id: "kids",
    label: "Kids",
    keywords: ["kids", "kid", "children", "child", "cartoon", "junior", "toon", "baby", "disney jr", "nick", "spacetoon", "اطفال", "أطفال", "كرتون", "سبيستون"],
  },
  {
    id: "religious",
    label: "Religious",
    keywords: [
      "islam", "islamic", "quran", "koran", "religious", "religion", "christian", "church", "majd", "hajj", "makkah", "mecca", "god",
      "اسلامي", "اسلاميه", "اسلامية", "الاسلامية", "الإسلامية", "قران", "قرآن", "القران", "القرآن", "المجد", "دينيه", "دينية", "مسيحي",
    ],
  },
  {
    id: "news",
    label: "News",
    keywords: ["news", "info", "journal", "nachrichten", "noticias", "notizie", "actualite", "اخبار", "أخبار", "الاخبار", "إخبارية", "اخباريه", "اخبارية"],
  },
  {
    id: "series",
    label: "Series",
    keywords: ["series", "serie", "séries", "drama", "dramas", "24/7", "247", "ramadan", "turkish series", "novela", "مسلسلات", "مسلسل", "دراما", "رمضان"],
  },
  {
    id: "movies",
    label: "Movies",
    keywords: [
      "movie", "movies", "film", "films", "cinema", "cinéma", "kino", "vod", "box office", "action", "comedy", "horror", "thriller",
      "osn", "hbo", "cine", "افلام", "أفلام", "فيلم", "سينما", "اكشن", "أكشن", "كوميدي", "رعب",
    ],
  },
  {
    id: "music",
    label: "Music",
    keywords: ["music", "musik", "musique", "songs", "radio", "hits", "mtv", "clip", "clips", "اغاني", "أغاني", "اغنية", "موسيقى", "موسيقي", "طرب", "مطربين"],
  },
  {
    id: "documentary",
    label: "Documentary",
    keywords: ["docu", "documentary", "documentaries", "discovery", "nat geo", "natgeo", "national geographic", "history", "animal", "science", "وثائقي", "وثائقيه", "وثائقية"],
  },
  {
    id: "entertainment",
    label: "Entertainment",
    keywords: ["entertainment", "general", "variety", "lifestyle", "family", "reality", "comedy central", "ترفيه", "ترفيهي", "ترفيهيه", "ترفيهية", "منوعات", "عامه", "عامة"],
  },
  { id: "events", label: "Events & PPV", keywords: ["ppv", "event", "events", "live events", "pay per view", "replay"] },
];

export const REGIONS: Facet[] = [
  {
    id: "arabic",
    label: "Arabic",
    prefixes: ["AR", "ARA", "ARB", "ARAB", "EG", "SA", "KSA", "AE", "UAE", "QA", "KW", "LB", "IQ", "JO", "SY", "MA", "DZ", "TN", "LY", "SD", "YE", "OM", "BH", "PS", "MENA"],
    keywords: [
      "arab", "arabic", "mbc", "rotana", "shahid", "alwan", "rabiaa", "majd", "osn", "egypt", "saudi", "ksa", "emirates", "uae", "dubai", "abu dhabi",
      "qatar", "kuwait", "lebanon", "syria", "iraq", "jordan", "palestine", "gulf", "khaleeji", "morocco", "maroc", "algeria", "algerie", "tunisia",
      "tunisie", "libya", "sudan", "yemen", "oman", "bahrain", "maghreb",
      "عربي", "عربيه", "عربية", "العربية", "مصر", "مصري", "مصريه", "السعودية", "السعوديه", "الامارات", "الإمارات", "قطر", "الكويت", "لبنان", "سوريا",
      "العراق", "الاردن", "الأردن", "فلسطين", "الخليج", "خليجي", "المغرب", "الجزائر", "تونس", "ليبيا", "السودان", "اليمن", "عمان", "البحرين",
    ],
  },
  {
    id: "english",
    label: "UK & US",
    prefixes: ["UK", "GB", "US", "USA", "CA", "CAN", "AU", "AUS", "NZ", "IE", "EN", "ENG"],
    keywords: ["english", "uk", "usa", "british", "britain", "american", "america", "canada", "australia", "ireland", "sky", "bbc", "itv", "fox", "nbc", "cbs", "abc", "اجنبي", "أجنبي", "انجليزي", "إنجليزي"],
  },
  {
    id: "french",
    label: "French",
    prefixes: ["FR", "FRA", "BE", "CH"],
    keywords: ["france", "french", "francais", "français", "belgique", "suisse", "canal+", "canal plus", "tf1", "فرنسي", "فرنسا"],
  },
  {
    id: "africa",
    label: "Africa",
    prefixes: ["AF", "AFR", "NG", "ZA", "KE", "GH", "SN", "CM", "CI", "ET", "TZ", "UG"],
    keywords: ["africa", "african", "afrique", "dstv", "gotv", "nigeria", "kenya", "ghana", "south africa", "senegal", "cameroon", "ethiopia", "افريقيا", "أفريقيا", "إفريقيا", "افريقي"],
  },
  {
    id: "asia",
    label: "Asia",
    prefixes: ["IN", "IND", "PK", "BD", "PH", "KR", "JP", "CN", "TH", "VN", "ID", "MY", "LK", "NP", "AF"],
    keywords: ["india", "indian", "hindi", "tamil", "telugu", "punjabi", "urdu", "pakistan", "bangla", "bangladesh", "asia", "asian", "korea", "korean", "japan", "japanese", "china", "chinese", "philippines", "pinoy", "thai", "vietnam", "indonesia", "malaysia", "هندي", "هندية", "باكستان", "اسيا", "آسيا", "كوري"],
  },
  {
    id: "turkish",
    label: "Turkish",
    prefixes: ["TR", "TUR"],
    keywords: ["turkey", "turkish", "türk", "turk", "turkiye", "türkiye", "تركيا", "تركي", "تركية"],
  },
  {
    id: "europe",
    label: "Europe",
    prefixes: ["DE", "GER", "AT", "IT", "ITA", "ES", "ESP", "PT", "NL", "PL", "GR", "RO", "RU", "SE", "NO", "DK", "FI", "HU", "CZ", "SK", "HR", "RS", "BG", "AL", "EX-YU", "EXYU", "BALKAN", "NORDIC"],
    keywords: ["germany", "german", "deutsch", "deutschland", "italy", "italia", "italian", "spain", "espana", "españa", "spanish", "portugal", "portuguese", "netherlands", "dutch", "poland", "polska", "greece", "greek", "romania", "russia", "russian", "nordic", "sweden", "norway", "denmark", "finland", "balkan", "ex-yu", "exyu", "albania", "serbia", "croatia", "bulgaria", "europe", "european", "المانيا", "ألمانيا", "ايطاليا", "إيطاليا", "اسبانيا", "إسبانيا"],
  },
  {
    id: "latin",
    label: "Latin America",
    prefixes: ["LAT", "MX", "BR", "AR-LAT", "CO", "CL", "PE"],
    keywords: ["latino", "latina", "latin", "latam", "mexico", "méxico", "brazil", "brasil", "argentina", "colombia", "chile", "peru"],
  },
];

/**
 * Fold for matching/search: lower-case, strip Latin diacritics and Arabic tashkeel/tatweel, unify
 * alef/hamza forms (أ إ آ ٱ → ا), ة → ه, ى → ي, ؤ → و, ئ → ي.
 */
export function foldText(s: string): string {
  return s
    .toLowerCase()
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[\u064B-\u065F\u0670\u0640]/g, "")
    .replace(/[\u0622\u0623\u0625\u0671]/g, "\u0627")
    .replace(/\u0629/g, "\u0647")
    .replace(/\u0649/g, "\u064A")
    .replace(/\u0624/g, "\u0648")
    .replace(/\u0626/g, "\u064A");
}

// Upper-case codes only, so "Africa - أفريقيا" keeps its name while "AR:", "UK|", "FR -", "[DE]" are tags.
const PREFIX_RE = /^\s*(?:[[(]([A-Z]{2,4}(?:-[A-Z]{2,4})?)[\])]\s*(?:[:|]|-\s|–\s)?|([A-Z]{2,4}(?:-[A-Z]{2,4})?)\s*(?:[:|]|-\s|–\s))/;

/** Leading tag like "AR:" / "UK|" / "FR -" / "[DE]" (upper-cased), if any. */
export function prefixOf(name: string): string | undefined {
  const m = PREFIX_RE.exec(name);
  return m ? (m[1] ?? m[2])!.toUpperCase() : undefined;
}

// Pre-folded keyword lists, longest first so "sky sport" beats "sky".
interface Compiled {
  f: Facet;
  words: string[];
  prefixes: Set<string>;
}
const compile = (l: Facet[]): Compiled[] =>
  l.map((f) => ({ f, words: f.keywords.map(foldText).sort((a, b) => b.length - a.length), prefixes: new Set(f.prefixes ?? []) }));
const TOPIC_C = compile(TOPICS);
const REGION_C = compile(REGIONS);

const isWordChar = (ch: string | undefined) => !!ch && /[a-z0-9\u0600-\u06FF]/.test(ch);

/** Whole-word (or Arabic word-start, allowing the ال article) match. */
function hasWord(hay: string, w: string): boolean {
  let i = hay.indexOf(w);
  while (i >= 0) {
    const before = hay[i - 1];
    const after = hay[i + w.length];
    const arabic = /[\u0600-\u06FF]/.test(w);
    const startOk = !isWordChar(before) || (arabic && hay.slice(Math.max(0, i - 2), i) === "\u0627\u0644");
    // Latin: whole word (allow plural "s"); Arabic: a word start is enough (suffixes vary).
    const endOk = arabic || !isWordChar(after) || (after === "s" && !isWordChar(hay[i + w.length + 1]));
    if (startOk && endOk) return true;
    i = hay.indexOf(w, i + 1);
  }
  return false;
}

function match(list: Compiled[], folded: string, prefix: string | undefined): Facet | undefined {
  if (prefix) for (const c of list) if (c.prefixes.has(prefix)) return c.f;
  for (const c of list) for (const w of c.words) if (hasWord(folded, w)) return c.f;
  return undefined;
}

export interface GroupInfo {
  name: string;
  count: number;
  folded: string;
  topic?: Facet;
  region?: Facet;
  /** A–Z bucket: A–Z, "ع" for Arabic script, "#" otherwise. */
  letter: string;
}

const cache = new Map<string, GroupInfo>();

export function classify(name: string, count: number): GroupInfo {
  const hit = cache.get(name);
  if (hit && hit.count === count) return hit;
  const folded = foldText(name);
  const prefix = prefixOf(name);
  // Topic ignores the region prefix ("AR: beIN Sports" → sports); region checks the prefix first.
  const topic = match(TOPIC_C, folded, undefined);
  // Bilingual names with Arabic script and no other region clue are aimed at Arabic viewers.
  const region = match(REGION_C, folded, prefix) ?? (/[\u0600-\u06FF]/.test(name) ? REGIONS.find((r) => r.id === "arabic") : undefined);
  const first = name.replace(PREFIX_RE, "").trim().charAt(0) || name.charAt(0);
  const letter = /[a-z]/i.test(first) ? first.toUpperCase() : /[\u0600-\u06FF]/.test(first) ? "ع" : "#";
  const g: GroupInfo = { name, count, folded, topic, region, letter };
  cache.set(name, g);
  return g;
}

/** Display name without the region tag ("AR: beIN Sports" → "beIN Sports"); keeps the original otherwise. */
export function shortName(name: string): string {
  const s = name.replace(PREFIX_RE, "").trim();
  return s.length >= 2 ? s : name;
}

/** Does a search word name this group's topic or region ("sport", "رياضة", "arabic", "عربي")? */
export function facetMatches(g: GroupInfo, word: string): boolean {
  if (word.length < 3) return false;
  const hit = (c: Compiled | undefined) =>
    !!c && (foldText(c.f.label).indexOf(word) === 0 || c.words.some((w) => w.indexOf(word) === 0 || (word.length >= 4 && word.indexOf(w) === 0)));
  return hit(TOPIC_C.find((c) => c.f === g.topic)) || hit(REGION_C.find((c) => c.f === g.region));
}
