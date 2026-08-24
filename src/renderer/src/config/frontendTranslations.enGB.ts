// British English UI translation strings. Shared wording stays sourced from the US catalog.
import { enUSTranslations } from './frontendTranslations.enUS'

export const enGBTranslations = {
  ...enUSTranslations,
  'startup.ambient.organizingContext': 'Organising workspace context',
  'sidebar.organizeSidebar': 'Organise sidebar',
  'sidebar.organizeByProject': 'By project',
  'sidebar.organizeByRecentProject': 'Recent projects',
  'chat.favorite': 'Favourite',
  'chat.favoriteMessage': 'Favourite message',
  'chat.unfavorite': 'Unfavourite',
  'chat.unfavoriteMessage': 'Remove message from favourites'
} as const
