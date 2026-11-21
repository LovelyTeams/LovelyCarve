import { File, Puzzle, ScrollText, History } from 'lucide-vue-next';
export function dataTypeIcon(dataType: number) {
  switch (dataType) {
    case 1:
      return File;
    case 2:
      return Puzzle;
    case 3:
      return ScrollText;
    default:
      return History;
  }
}
