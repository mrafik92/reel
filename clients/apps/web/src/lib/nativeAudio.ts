/** Find the native track for the server's chosen audio in a direct-play file. */
export function nativeAudioChoice(
  mode: string,
  audioType: string,
  streams: { index: number }[],
  selected: number | null,
  tracks: { id: string; selected?: boolean }[],
): string | undefined {
  if (mode !== "direct_play" || audioType !== "copy" || tracks.length !== streams.length) return undefined;
  const position = streams.findIndex((stream) => stream.index === selected);
  const track = tracks[position];
  return track && !track.selected ? track.id : undefined;
}
