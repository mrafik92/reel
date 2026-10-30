import { render } from "solid-js/web";
import { lazy } from "solid-js";
import { HashRouter, Route, Router } from "@solidjs/router";
import "./styles/fonts.css";
import "@reel/ui/src/styles/ui.css";
import "./styles/app.css";
import { HASH_ROUTING, TV } from "./lib/mode";
import { Shell } from "./app/Shell";
import Home from "./pages/Home";
import Library from "./pages/Library";
import Item from "./pages/Item";

const Player = lazy(() => import("./pages/Player"));
const Search = lazy(() => import("./pages/Search"));
const DevRings = lazy(() => import("./pages/DevRings"));
const NotFound = lazy(() => import("./pages/NotFound"));

const root = document.documentElement;
if (TV) root.classList.add("tv");

function Routes() {
  return (
    <>
      <Route path="/" component={Home} />
      <Route path="/movies" component={() => <Library kind="movies" />} />
      <Route path="/shows" component={() => <Library kind="shows" />} />
      <Route path="/item/:id" component={Item} />
      <Route path="/play/:id" component={Player} />
      <Route path="/search" component={Search} />
      <Route path="/dev/rings" component={DevRings} />
      <Route path="*" component={NotFound} />
    </>
  );
}

const R = HASH_ROUTING ? HashRouter : Router;
render(() => <R root={Shell}>{Routes()}</R>, document.getElementById("root")!);
