/**
 * Navigation to query dispatch.
 *
 * The sidebar selection decides which query the content view runs. This module
 * holds that decision only — it is pure, so the mapping can be tested without
 * a database. Content.vue executes the plan (titles, requests, side effects).
 *
 * The enum values mirror src/common/constants.ts. They are copied because this
 * module is plain JavaScript and the tests import it without a bundler; the
 * test file compares both sources so they cannot drift apart.
 */

import { normalizeAiSearchMode } from './aiSearch.js';

export const SIDEBAR = {
  LIBRARY: 0,
  ALBUM: 1,
  SMART_ALBUM: 2,
  SEARCH: 3,
  TAG: 4,
  CALENDAR: 5,
  PERSON: 6,
  LOCATION: 7,
  CAMERA: 8,
  AI_SEARCH: 9,
};

export const LIB_ITEM = {
  ALL: 'all-files',
  FAV: 'favorites',
  TODAY: 'on-this-day',
  RATINGS: 'ratings',
  CULLING: 'culling',
  SUBJECTS: 'subjects',
  VL_ANNOTATED: 'vl-annotated',
  VL_UNANNOTATED: 'vl-unannotated',
};

export const RATE = {
  ALL: -2,
  NONE: -1,
  UNRATED: 0,
};

export const CULLING = {
  PICK: 'pick',
  REJECT: 'reject',
  UNREVIEWED: 'unreviewed',
};

export const DATE_SORT = {
  TAKEN_DESC: 1,
};

/** What the content view has to do for a plan. */
export const QUERY_KIND = {
  FILES: 'files',             // a file query with params
  IMAGE_SEARCH: 'image-search', // a vector search over a prompt
  SMART_ALBUM: 'smart-album',   // a smart album query
  UNIFIED_SEARCH: 'unified-search', // the combined search
  AI_SEARCH: 'ai-search',     // the image-understanding search entry
  EMPTY: 'empty',             // no results, show the empty state
  BLANK: 'blank',             // no title, no query
  ALBUM: 'album',             // the album record has to be loaded first
  TAG: 'tag',                 // the tag name has to be loaded first
  NONE: 'none',               // nothing to do
};

/** How the title of the content area is built. */
export const TITLE_KIND = {
  BLANK: 'blank',
  LOCALE: 'locale',
  LOCALE_PAIR: 'locale-pair',
  TEXT: 'text',
  LOCALE_OR_TEXT: 'locale-or-text',
  RATING: 'rating',
  SUBJECT: 'subject',
  ALBUM_NAME: 'album-name',
  BREADCRUMB: 'breadcrumb',
  CALENDAR: 'calendar',
  SMART_ALBUM: 'smart-album',
  SEARCH: 'search',
  AI_SEARCH: 'ai-search',
  TAG_NAME: 'tag-name',
};

/**
 * Plan the query of the current sidebar selection.
 *
 * The state holds the current selection of every panel; the plan holds the
 * query that runs for it.
 */
export function planNavigationQuery(state = {}) {
  switch (Number(state.sidebarIndex)) {
    case SIDEBAR.LIBRARY:
      return planLibraryQuery(state);
    case SIDEBAR.ALBUM:
      return planAlbumQuery(state);
    case SIDEBAR.SMART_ALBUM:
      return planSmartAlbumQuery(state);
    case SIDEBAR.SEARCH:
      return planSearchQuery(state);
    case SIDEBAR.CALENDAR:
      return planCalendarQuery(state);
    case SIDEBAR.TAG:
      return planTagQuery(state);
    case SIDEBAR.PERSON:
      return planPersonQuery(state);
    case SIDEBAR.LOCATION:
      return planLocationQuery(state);
    case SIDEBAR.CAMERA:
      return planCameraQuery(state);
    case SIDEBAR.AI_SEARCH:
      return planAiSearchQuery(state);
    default:
      return { kind: QUERY_KIND.NONE, title: { type: TITLE_KIND.BLANK } };
  }
}

/** The library panel: favorites, on this day, ratings, culling, subjects. */
function planLibraryQuery(state) {
  const library = state.library || {};

  switch (library.item) {
    case LIB_ITEM.FAV:
      return {
        kind: QUERY_KIND.FILES,
        params: { isFavorite: true },
        title: { type: TITLE_KIND.LOCALE, key: 'favorite.files' },
      };

    case LIB_ITEM.TODAY:
      return {
        kind: QUERY_KIND.FILES,
        params: { startDate: -1, endDate: -1, calendarSort: DATE_SORT.TAKEN_DESC },
        title: { type: TITLE_KIND.LOCALE, key: 'library.on_this_day' },
      };

    case LIB_ITEM.RATINGS: {
      const rating = state.rating?.item;
      if (rating === RATE.ALL) {
        return {
          kind: QUERY_KIND.FILES,
          params: { rating: RATE.ALL },
          title: { type: TITLE_KIND.LOCALE_PAIR, key: 'rating.title', subKey: 'rating.rated' },
        };
      }
      if (rating === RATE.UNRATED) {
        return {
          kind: QUERY_KIND.FILES,
          params: { rating: RATE.UNRATED },
          title: { type: TITLE_KIND.LOCALE_PAIR, key: 'rating.title', subKey: 'rating.unrated' },
        };
      }
      const value = Number(rating || 0);
      if (value > 0) {
        return {
          kind: QUERY_KIND.FILES,
          params: { rating: value },
          title: { type: TITLE_KIND.RATING, rating: value },
        };
      }
      return {
        kind: QUERY_KIND.EMPTY,
        title: { type: TITLE_KIND.LOCALE, key: 'rating.title' },
      };
    }

    case LIB_ITEM.CULLING: {
      const item = state.culling?.item;
      const cullingFlag = item === CULLING.PICK ? 1 : item === CULLING.REJECT ? 2 : 0;
      const subKey = item === CULLING.PICK
        ? 'culling.picks'
        : item === CULLING.REJECT
          ? 'culling.rejected'
          : 'culling.unreviewed';
      return {
        kind: QUERY_KIND.FILES,
        params: { cullingFlag },
        title: { type: TITLE_KIND.LOCALE_PAIR, key: 'culling.title', subKey },
      };
    }

    case LIB_ITEM.VL_ANNOTATED:
      return {
        kind: QUERY_KIND.FILES,
        params: { vlAnnotated: true },
        title: { type: TITLE_KIND.LOCALE, key: 'aiSearch.vl_annotated' },
      };

    case LIB_ITEM.VL_UNANNOTATED:
      return {
        kind: QUERY_KIND.FILES,
        params: { vlAnnotated: false },
        title: { type: TITLE_KIND.LOCALE, key: 'aiSearch.vl_unannotated' },
      };

    case LIB_ITEM.SUBJECTS: {
      const smartTag = library.smartTag || null;
      if (!library.smartId || !smartTag) {
        return {
          kind: QUERY_KIND.EMPTY,
          title: { type: TITLE_KIND.LOCALE, key: 'subject.title' },
        };
      }
      return {
        kind: QUERY_KIND.IMAGE_SEARCH,
        prompt: smartTag.prompt,
        threshold: smartTag.threshold ?? null,
        smartTagId: smartTag.id,
        title: { type: TITLE_KIND.SUBJECT, smartTagId: smartTag.id },
      };
    }

    default:
      return {
        kind: QUERY_KIND.FILES,
        params: {},
        title: { type: TITLE_KIND.LOCALE, key: 'library.all_files' },
      };
  }
}

/** The album panel: the whole library, an album or one of its folders. */
function planAlbumQuery(state) {
  const album = state.album || {};

  if (album.id === null) {
    return { kind: QUERY_KIND.BLANK, title: { type: TITLE_KIND.BLANK } };
  }
  if (album.id === 0) {
    return {
      kind: QUERY_KIND.FILES,
      params: {},
      title: { type: TITLE_KIND.LOCALE, key: 'library.all_files' },
    };
  }

  const record = album.record || null;
  if (!record) {
    // The album record is loaded by the caller, then the plan runs again.
    return { kind: QUERY_KIND.ALBUM, title: { type: TITLE_KIND.BLANK } };
  }

  if (album.selected) {
    return {
      kind: QUERY_KIND.FILES,
      params: { searchAllSubfolders: record.path },
      title: { type: TITLE_KIND.ALBUM_NAME, name: record.name || '' },
    };
  }

  const folderPath = album.folderPath || '';
  const params = album.showSubfolderFiles
    ? { searchAllSubfolders: folderPath }
    : { searchFolder: folderPath };
  return {
    kind: QUERY_KIND.FILES,
    params,
    title: { type: TITLE_KIND.BREADCRUMB, folderPath, albumPath: record.path },
  };
}

/** The smart album panel: a user defined smart album. */
function planSmartAlbumQuery(state) {
  const smartAlbum = state.smartAlbum || {};
  if (smartAlbum.type === 'custom' && smartAlbum.id) {
    const item = (smartAlbum.items || []).find((entry) => entry.id === smartAlbum.id);
    if (item) {
      return {
        kind: QUERY_KIND.SMART_ALBUM,
        smartAlbum: item,
        title: { type: TITLE_KIND.SMART_ALBUM, name: item.name || '' },
      };
    }
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }
  return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
}

/** The search panel: the combined search. */
function planSearchQuery(state) {
  const text = state.search?.text || '';
  if (text) {
    return {
      kind: QUERY_KIND.UNIFIED_SEARCH,
      text,
      title: { type: TITLE_KIND.SEARCH, text },
    };
  }
  return {
    kind: QUERY_KIND.EMPTY,
    title: { type: TITLE_KIND.LOCALE, key: 'search.title' },
  };
}

/**
 * The AI search panel: BM25 and/or the caption embeddings.
 *
 * The mode comes from `config.settings.aiSearchMode`; the official
 * `searchMode` setting belongs to the unified search and is never read here.
 */
function planAiSearchQuery(state) {
  const text = state.search?.text || '';
  const mode = normalizeAiSearchMode(state.aiSearch?.mode);
  if (text) {
    return {
      kind: QUERY_KIND.AI_SEARCH,
      text,
      mode,
      title: { type: TITLE_KIND.AI_SEARCH, text },
    };
  }
  return {
    kind: QUERY_KIND.EMPTY,
    title: { type: TITLE_KIND.LOCALE, key: 'aiSearch.title' },
  };
}

/** The calendar panel: the files of the selected day, month or year. */
function planCalendarQuery(state) {
  const calendar = state.calendar || {};
  if (calendar.year === null) {
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }

  let format = 'date_long';
  if (Number(calendar.month) === -1) {
    format = 'year';
  } else if (Number(calendar.date) === -1) {
    format = 'month';
  }

  return {
    kind: QUERY_KIND.FILES,
    params: { startDate: calendar.startDate, endDate: calendar.endDate },
    title: {
      type: TITLE_KIND.CALENDAR,
      year: calendar.year,
      month: calendar.month,
      date: calendar.date,
      format,
    },
  };
}

/** The tag panel: the files of the selected tag. */
function planTagQuery(state) {
  const tag = state.tag || {};
  if (tag.id === null) {
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }
  return {
    kind: QUERY_KIND.TAG,
    tagId: tag.id,
    params: { tagId: tag.id || 0 },
    title: { type: TITLE_KIND.TAG_NAME, tagId: tag.id },
  };
}

/** The person panel: the files of the selected person. */
function planPersonQuery(state) {
  const person = state.person || {};
  if (person.id === null) {
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }
  return {
    kind: QUERY_KIND.FILES,
    params: { personId: person.id },
    title: { type: TITLE_KIND.LOCALE_OR_TEXT, key: 'sidebar.people', text: person.name || '' },
  };
}

/** The location panel: the files of the selected place. */
function planLocationQuery(state) {
  const location = state.location || {};
  if (location.admin1 === null) {
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }
  if (location.name) {
    return {
      kind: QUERY_KIND.FILES,
      params: { locationAdmin1: location.admin1, locationName: location.name },
      title: { type: TITLE_KIND.TEXT, text: `${location.admin1} > ${location.name}` },
    };
  }
  return {
    kind: QUERY_KIND.FILES,
    params: { locationAdmin1: location.admin1 },
    title: { type: TITLE_KIND.TEXT, text: `${location.admin1}` },
  };
}

/** The camera panel: the files of the selected camera or lens. */
function planCameraQuery(state) {
  const camera = state.camera || {};

  if (!camera.isCamera) {
    if (camera.lensMake === null) {
      return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
    }
    if (camera.lensModel) {
      return {
        kind: QUERY_KIND.FILES,
        params: { lensMake: camera.lensMake, lensModel: camera.lensModel },
        title: { type: TITLE_KIND.TEXT, text: `${camera.lensMake} > ${camera.lensModel}` },
      };
    }
    return {
      kind: QUERY_KIND.FILES,
      params: { lensMake: camera.lensMake },
      title: { type: TITLE_KIND.TEXT, text: `${camera.lensMake}` },
    };
  }

  if (camera.make === null) {
    return { kind: QUERY_KIND.EMPTY, title: { type: TITLE_KIND.BLANK } };
  }
  if (camera.model) {
    return {
      kind: QUERY_KIND.FILES,
      params: { make: camera.make, model: camera.model },
      title: { type: TITLE_KIND.TEXT, text: `${camera.make} > ${camera.model}` },
    };
  }
  return {
    kind: QUERY_KIND.FILES,
    params: { make: camera.make },
    title: { type: TITLE_KIND.TEXT, text: `${camera.make}` },
  };
}
