/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

import { createSignal } from 'solid-js';

import { CACHE_DIR } from '@diffusionstudio/assets';

import { createProjectFS } from './fs';
import { getProject, isDesktop } from './host';

/** Where the snapshot lives inside the project folder. */
export const COVER_PATH = `${CACHE_DIR}/cover.png`;

/** How many covers are kept in hand — enough for the projects one session closes. */
const RECENT_LIMIT = 8;

/** Captures under way, by project folder. */
const pending = new Map<string, Promise<void>>();
/** Covers taken this session, by project folder: the file, without the trip to disk. */
const recent = new Map<string, Blob>();
/** How often each project's cover has been replaced this session. */
const [revisions, setRevisions] = createSignal<Record<string, number>>({});

/**
 * Takes the cover of the project in `dir` from `snapshot` and writes it.
 * Call it as the project closes, with the snapshot already under way: the
 * capture is registered synchronously, so a read that follows waits for this
 * cover rather than answering with the last one.
 */
export function captureProjectCover(dir: string, snapshot: Promise<Blob | null>): Promise<void> {
	if (!dir || !isDesktop()) return Promise.resolve();

	// Until the new one arrives there is no cover in hand: what is there
	// shows the project as it was before this session's edits.
	recent.delete(dir);

	const done = capture(dir, snapshot)
		.catch((error) => console.warn('[projects] could not write the project cover', error))
		.finally(() => {
			if (pending.get(dir) === done) pending.delete(dir);
		});

	pending.set(dir, done);
	return done;
}

/**
 * What to key a read of the project's cover on: a value that changes once a
 * new cover has been taken for `dir`, and stays put when another project's
 * cover is replaced. Read it as a resource's source and the read runs again
 * when the cover lands — which is what a card asking for the cover of a
 * project still closing needs.
 */
export function projectCoverKey(dir: string): string {
	return `${revisions()[dir] ?? 0}:${dir}`;
}

/** The cover of the project in `dir`, or null while it has none. */
export async function readProjectCover(dir: string): Promise<Blob | null> {
	if (!dir || !isDesktop()) return null;

	if (!recent.has(dir)) await pending.get(dir);

	const captured = recent.get(dir);
	if (captured) return captured;

	try {
		const fs = createProjectFS(dir);
		// Asked for before it is read: a missing cover is the normal case
		// (every project that has not been opened yet), not a failed read.
		if (!(await fs.stat(COVER_PATH))) return null;
		return await fs.file(COVER_PATH);
	} catch (error) {
		console.warn('[projects] could not read the project cover', error);
		return null;
	}
}

async function capture(dir: string, snapshot: Promise<Blob | null>): Promise<void> {
	const blob = await snapshot;
	if (!blob) return;

	// A project closed by deleting it is already in the trash, and writing
	// would put its folder back — holding nothing but this one file.
	if (!(await getProject(dir))) return;

	await createProjectFS(dir).write(COVER_PATH, blob);

	remember(dir, blob);
	setRevisions((current) => ({ ...current, [dir]: (current[dir] ?? 0) + 1 }));
}

function remember(dir: string, blob: Blob): void {
	recent.set(dir, blob);
	// Insertion order, so the cover taken longest ago is the first to go.
	for (const key of recent.keys()) {
		if (recent.size <= RECENT_LIMIT) break;
		recent.delete(key);
	}
}
