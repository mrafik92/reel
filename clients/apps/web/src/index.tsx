import "./lib/startupTiming";
import { rememberPoster } from "./lib/posterMotion";
import { render } from "solid-js/web";
import { lazy } from "solid-js";
import { HashRouter, Route, Router } from "@solidjs/router";
import "./styles/fonts.css";
import "@reel/ui/src/styles/ui.css";
import "./styles/app.css";
import "./styles/features.css";
import "./styles/people.css";
import { HASH_ROUTING, TV } from "./lib/mode";
import { Shell } from "./app/Shell";
import Home from "./pages/Home";
import Library from "./pages/Library";
import Item from "./pages/Item";
import { Connect, Invite, Login, Setup } from "./pages/Auth";

const Player = lazy(() => import("./pages/Player"));
const Search = lazy(() => import("./pages/Search"));
const Discover = lazy(() => import("./pages/Discover"));
const Requests = lazy(() => import("./pages/Requests"));
const Downloads = lazy(() => import("./pages/Downloads"));
const Person = lazy(() => import("./pages/Person"));
const Admin = lazy(() => import("./pages/admin/Admin"));
const LiveTV = lazy(() => import("./pages/LiveTV"));
const LiveGuide = lazy(() => import("./pages/LiveGuide"));
const VodMovie = lazy(() => import("./pages/VodMovie"));
const VodSeries = lazy(() => import("./pages/VodSeries"));
const VodPlayer = lazy(() => import("./pages/VodPlayer"));
const DevRings = lazy(() => import("./pages/DevRings"));
const NotFound = lazy(() => import("./pages/NotFound"));

const root = document.documentElement;
if (TV) root.classList.add("tv");

function Routes() {
  return (
    <>
      <Route path="/login" component={Login} />
      <Route path="/setup" component={Setup} />
      <Route path="/connect" component={Connect} />
      <Route path="/invite/:code" component={Invite} />
      <Route path="/" component={Home} />
      <Route path="/movies" component={() => <Library kind="movies" />} />
      <Route path="/shows" component={() => <Library kind="shows" />} />
      <Route path="/item/:id" component={Item} />
      <Route path="/play/:id" component={Player} />
      <Route path="/search" component={Search} />
      <Route path="/discover" component={Discover} />
      <Route path="/requests" component={Requests} />
      <Route path="/downloads" component={Downloads} />
      <Route path="/person/:id" component={Person} />
      <Route path="/live" component={LiveTV} />
      <Route path="/live/guide" component={LiveGuide} />
      <Route path="/vod/movie/:id" component={VodMovie} />
      <Route path="/vod/series/:id" component={VodSeries} />
      <Route path="/vod/play/:kind/:id" component={VodPlayer} />
      <Route path="/settings/playback" component={() => <Admin section="playback" />} />
      <Route path="/settings/account" component={() => <Admin section="account" />} />
      <Route path="/admin/:section?" component={Admin} />
      <Route path="/dev/rings" component={DevRings} />
      <Route path="*" component={NotFound} />
    </>
  );
}

document.addEventListener("click", rememberPoster, true);

const R = HASH_ROUTING ? HashRouter : Router;
render(() => <R root={Shell}>{Routes()}</R>, document.getElementById("root")!);
